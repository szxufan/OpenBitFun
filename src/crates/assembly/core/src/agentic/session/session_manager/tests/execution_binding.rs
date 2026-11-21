use super::super::{SessionExecutionBindingError, SessionExecutionBindingUpdate};
use super::support::{in_memory_test_manager, test_manager, TestWorkspace};
use crate::agentic::core::{SessionAgentRouteOwner, SessionConfig};
use crate::agentic::persistence::PersistenceManager;
use crate::agentic::session::revert::{
    SessionRevertPhase, SessionRevertState, SESSION_REVERT_SCHEMA_VERSION,
};
use crate::service::session::{DialogTurnData, UserMessageData};
use openbitfun_core_types::SessionExecutionTarget;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn execution_binding_rejects_a_session_after_its_first_turn() {
    let manager = in_memory_test_manager();
    let workspace = TestWorkspace::new();
    let session = manager
        .create_session(
            "Binding race".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should be created");
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("session should remain loaded")
        .dialog_turn_ids
        .push("turn-1".to_string());

    let error = manager
        .update_session_execution_binding(
            &session.session_id,
            SessionExecutionBindingUpdate {
                workspace_path: "/tmp/worktree".to_string(),
                project_workspace_path: workspace.path().to_string_lossy().to_string(),
                workspace_id: None,
                execution_target: SessionExecutionTarget::local("/tmp/worktree".to_string()),
            },
        )
        .await
        .expect_err("a non-empty session must not move");

    assert!(matches!(error, SessionExecutionBindingError::Busy(_)));
    assert_eq!(
        manager
            .get_session(&session.session_id)
            .and_then(|session| session.config.workspace_path),
        Some(workspace.path().to_string_lossy().to_string())
    );
}

#[tokio::test]
async fn execution_binding_restores_a_view_only_empty_session_from_its_project() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "View-only binding".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                project_workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should be created");
    assert!(manager
        .unload_session_from_memory(&session.session_id)
        .await
        .expect("session should unload"));
    manager
        .session_storage_path_index
        .remove(&session.session_id);

    let target_path = workspace.path().join("managed-worktree");
    manager
        .update_session_execution_binding(
            &session.session_id,
            SessionExecutionBindingUpdate {
                workspace_path: target_path.to_string_lossy().to_string(),
                project_workspace_path: workspace.path().to_string_lossy().to_string(),
                workspace_id: Some("workspace-2".to_string()),
                execution_target: SessionExecutionTarget::local(
                    target_path.to_string_lossy().to_string(),
                ),
            },
        )
        .await
        .expect("view-only session should restore and rebind");

    let restored = manager
        .get_session(&session.session_id)
        .expect("session should be loaded after rebinding");
    assert_eq!(
        restored.config.workspace_path.as_deref(),
        Some(target_path.to_string_lossy().as_ref())
    );
    assert_eq!(restored.config.workspace_id.as_deref(), Some("workspace-2"));
}

#[tokio::test]
async fn execution_binding_rejects_a_boundary_zero_revert_after_explicit_restore() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Boundary zero binding".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                project_workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should be created");
    let storage_path = manager
        .effective_session_storage_path(&session.session_id)
        .await
        .expect("storage path");
    persistence_manager
        .save_dialog_turn(
            &storage_path,
            &DialogTurnData::new(
                "turn-0".to_string(),
                0,
                session.session_id.clone(),
                UserMessageData {
                    id: "user-0".to_string(),
                    content: "first prompt".to_string(),
                    timestamp: 1,
                    metadata: None,
                },
            ),
        )
        .await
        .expect("persist first turn");
    persistence_manager
        .save_session_revert_state(
            &storage_path,
            &session.session_id,
            &SessionRevertState {
                schema_version: SESSION_REVERT_SCHEMA_VERSION,
                boundary_turn: 0,
                original_turn_end: 1,
                phase: SessionRevertPhase::Staged,
                workspace_checkpoint: Vec::new(),
            },
        )
        .await
        .expect("persist boundary zero marker");
    assert!(manager
        .unload_session_from_memory(&session.session_id)
        .await
        .expect("unload session"));
    let restored = manager
        .restore_session_from_storage_path(&storage_path, &session.session_id)
        .await
        .expect("restore from the known Session storage path");
    assert!(
        restored.dialog_turn_ids.is_empty(),
        "boundary-zero restore must project an empty visible history"
    );

    let target_path = workspace.path().join("managed-worktree");
    let error = manager
        .update_session_execution_binding(
            &session.session_id,
            SessionExecutionBindingUpdate {
                workspace_path: target_path.to_string_lossy().to_string(),
                project_workspace_path: workspace.path().to_string_lossy().to_string(),
                workspace_id: Some("workspace-2".to_string()),
                execution_target: SessionExecutionTarget::local(
                    target_path.to_string_lossy().to_string(),
                ),
            },
        )
        .await
        .expect_err("a staged revert must pin its snapshot workspace");

    assert!(
        matches!(error, SessionExecutionBindingError::Busy(_)),
        "expected staged revert to reject rebinding as busy, got {error:?}"
    );
    assert_eq!(
        manager
            .get_session(&session.session_id)
            .expect("session remains restored after rejection")
            .config
            .workspace_path
            .as_deref(),
        Some(workspace.path().to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn same_session_mode_is_a_timestamp_preserving_noop() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Idempotent mode update".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let before = manager
        .get_session(&session.session_id)
        .expect("active session before update");
    let before_updated_at = before.updated_at;
    let before_last_activity_at = before.last_activity_at;
    drop(before);
    tokio::time::sleep(Duration::from_millis(20)).await;

    manager
        .update_session_agent_type(&session.session_id, "Standard")
        .await
        .expect("same mode should succeed");

    let after = manager
        .get_session(&session.session_id)
        .expect("active session after update");
    assert_eq!(after.updated_at, before_updated_at);
    assert_eq!(after.last_activity_at, before_last_activity_at);
}

#[tokio::test]
async fn session_mode_persists_without_a_turn_and_survives_restore() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Durable mode update".to_string(),
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
        .update_session_agent_type(&session.session_id, "Cowork")
        .await
        .expect("mode update should persist without a turn");
    let metadata = persistence_manager
        .load_session_metadata(workspace.path(), &session.session_id)
        .await
        .expect("metadata should load")
        .expect("metadata should exist");
    assert_eq!(metadata.agent_type, "Cowork");

    manager.evict_loaded_session_for_test(&session.session_id);
    let restored = manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("session should restore");
    assert_eq!(restored.agent_type, "Cowork");
}

#[tokio::test]
async fn external_agent_binding_persists_atomically_and_never_restores_as_local_by_name() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Durable external route".to_string(),
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
        .update_session_agent_binding(
            &session.session_id,
            "Standard",
            SessionAgentRouteOwner::External,
            Some("test:external:agentic".to_string()),
        )
        .await
        .expect("same-id local-to-external rebind should persist");
    assert_eq!(
        manager
            .get_session(&session.session_id)
            .expect("rebound session")
            .config
            .agent_route_owner,
        SessionAgentRouteOwner::External
    );
    manager
        .update_session_agent_binding(
            &session.session_id,
            "Standard",
            SessionAgentRouteOwner::Local,
            Some("local:agentic".to_string()),
        )
        .await
        .expect("same-id external-to-local rebind should persist");
    assert_eq!(
        manager
            .get_session(&session.session_id)
            .expect("rebound session")
            .config
            .agent_route_owner,
        SessionAgentRouteOwner::Local
    );

    manager
        .update_session_agent_binding(
            &session.session_id,
            "Cowork",
            SessionAgentRouteOwner::External,
            Some("test:external:plan".to_string()),
        )
        .await
        .expect("external route update should persist without a turn");

    let (persisted, _) = persistence_manager
        .load_session_with_turns(workspace.path(), &session.session_id)
        .await
        .expect("persisted session should load");
    assert_eq!(persisted.agent_type, "Cowork");
    assert_eq!(
        persisted.config.agent_route_owner,
        SessionAgentRouteOwner::External
    );

    manager.evict_loaded_session_for_test(&session.session_id);
    let restored = manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("external route should restore fail-closed");
    assert_eq!(restored.agent_type, "Cowork");
    assert_eq!(
        restored.config.agent_route_owner,
        SessionAgentRouteOwner::External,
        "a same-name local mode must not capture a persisted external route"
    );
}

#[tokio::test]
async fn session_mode_update_does_not_rewrite_the_runtime_state_file() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Metadata-only mode update".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    persistence_manager.fail_next_session_state_write_for_test(&session.session_id);

    manager
        .update_session_agent_type(&session.session_id, "Cowork")
        .await
        .expect("mode updates must not depend on rewriting runtime state");
    manager.evict_loaded_session_for_test(&session.session_id);

    let restored = manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("metadata-only mode update should remain restorable");
    assert_eq!(restored.agent_type, "Cowork");
}
