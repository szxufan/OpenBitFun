use super::super::SessionManager;
use super::support::{test_manager, TestWorkspace};
use crate::agentic::core::{Message, SessionConfig};
use crate::agentic::persistence::PersistenceManager;
use crate::agentic::session::revert::{
    SessionRevertPhase, SessionRevertState, SESSION_REVERT_SCHEMA_VERSION,
};
use crate::util::errors::OpenBitFunError;
use std::sync::Arc;

#[tokio::test]
async fn delete_session_removes_workspace_cache_entry() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Cached session".to_string(),
            "agent".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let session_storage_dir = persistence_manager
        .path_manager()
        .project_sessions_dir(workspace.path());
    assert!(session_storage_dir.exists());
    let expected_storage_path =
        SessionManager::normalize_session_storage_path(&session_storage_dir);

    assert_eq!(
        manager
            .session_storage_path_index
            .get(&session.session_id)
            .as_deref()
            .map(|entry| entry.path.clone()),
        Some(expected_storage_path)
    );

    manager
        .delete_session(workspace.path(), &session.session_id)
        .await
        .expect("session should delete");

    assert!(manager
        .session_storage_path_index
        .get(&session.session_id)
        .is_none());
}

#[tokio::test]
async fn delete_session_accepts_an_already_resolved_sessions_directory() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let resolved_sessions_dir = persistence_manager
        .path_manager()
        .project_sessions_dir(workspace.path());
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Resolved storage session".to_string(),
            "agent".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    manager
        .delete_session(&resolved_sessions_dir, &session.session_id)
        .await
        .expect("resolved sessions path should be idempotent");

    assert!(manager.get_session(&session.session_id).is_none());
    assert!(!resolved_sessions_dir.join(&session.session_id).exists());
}

#[tokio::test]
async fn evicted_session_uses_persisted_workspace_identity_for_snapshot_cleanup() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let resolved_sessions_dir = persistence_manager
        .path_manager()
        .project_sessions_dir(workspace.path());
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Evicted cleanup session".to_string(),
            "agent".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    manager.evict_loaded_session_for_test(&session.session_id);

    let cleanup_workspace_path = manager
        .resolve_session_cleanup_workspace_path(
            &resolved_sessions_dir,
            &session.session_id,
            &resolved_sessions_dir,
        )
        .await;

    assert_eq!(
        dunce::canonicalize(cleanup_workspace_path).expect("cleanup workspace should exist"),
        dunce::canonicalize(workspace.path()).expect("workspace should exist")
    );
}

#[tokio::test]
async fn delete_session_rejects_a_loaded_session_from_another_workspace() {
    let workspace = TestWorkspace::new();
    let other_workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Bound session".to_string(),
            "agent".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    let error = manager
        .delete_session(other_workspace.path(), &session.session_id)
        .await
        .expect_err("cross-workspace deletion must be rejected");

    assert!(error.to_string().contains("another workspace"));
    assert!(manager.get_session(&session.session_id).is_some());
    assert!(manager
        .session_storage_path_index
        .contains_key(&session.session_id));
}

#[tokio::test]
async fn persistence_delete_failure_preserves_loaded_runtime_context() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Failure atomic session".to_string(),
            "agent".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    manager.context_store.add_message(
        &session.session_id,
        Message::user("runtime context must survive".to_string()),
    );
    let storage_path = manager
        .session_storage_path_index
        .get(&session.session_id)
        .expect("storage binding")
        .path
        .clone();
    let index_path = storage_path.join("index.json");
    std::fs::remove_file(&index_path).expect("replace index file");
    std::fs::create_dir(&index_path).expect("create invalid index directory");

    manager
        .delete_session(workspace.path(), &session.session_id)
        .await
        .expect_err("persistence failure should abort runtime cleanup");

    assert!(manager.get_session(&session.session_id).is_some());
    assert_eq!(
        manager
            .context_store
            .get_context_messages(&session.session_id)
            .len(),
        1
    );
    assert!(manager
        .session_storage_path_index
        .contains_key(&session.session_id));
}

#[tokio::test]
async fn direct_delete_rejects_an_unfinished_revert_transition() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Pending revert delete".to_string(),
            "agent".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let state = SessionRevertState {
        schema_version: SESSION_REVERT_SCHEMA_VERSION,
        boundary_turn: 0,
        original_turn_end: 1,
        phase: SessionRevertPhase::Applying,
        workspace_checkpoint: Vec::new(),
    };
    persistence_manager
        .save_session_revert_state(workspace.path(), &session.session_id, &state)
        .await
        .expect("pending marker should persist");

    let error = manager
        .delete_session(workspace.path(), &session.session_id)
        .await
        .expect_err("direct deletion must not discard recovery state");

    assert!(matches!(error, OpenBitFunError::OutcomeUnknown(_)));
    assert!(manager.get_session(&session.session_id).is_some());
    assert_eq!(
        persistence_manager
            .load_session_revert_state(workspace.path(), &session.session_id)
            .await
            .expect("marker load"),
        Some(state)
    );
}

#[tokio::test]
async fn cold_session_deletion_reclaims_persisted_file_snapshots() {
    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let persistence = Arc::new(PersistenceManager::new(paths.clone()).unwrap());
    let manager = test_manager(persistence);
    for simulate_busy_writer in [false, true] {
        let session = manager
            .create_session(
                "Cold".into(),
                "agent".into(),
                SessionConfig {
                    workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let id = &session.session_id;
        let snapshots = paths.project_snapshots_dir(workspace.path());
        let ops = snapshots.join("operations");
        std::fs::create_dir_all(&ops).unwrap();
        let history = ops.join(format!("{id}.json"));
        std::fs::write(
            &history,
            serde_json::json!({
                "session_id": id, "last_updated": std::time::SystemTime::now(), "turns": {}
            })
            .to_string(),
        )
        .unwrap();
        manager.evict_loaded_session_for_test(id);
        assert!(
            crate::service::snapshot::get_snapshot_manager_for_workspace(workspace.workspace_id())
                .is_none()
        );
        let gate = if simulate_busy_writer {
            let dir = paths
                .project_runtime_root(workspace.path())
                .join("locks/snapshot-writers");
            std::fs::create_dir_all(&dir).unwrap();
            let gate = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join("maintenance.lock"))
                .unwrap();
            gate.try_lock().unwrap();
            Some(gate)
        } else {
            None
        };
        manager.delete_session(workspace.path(), id).await.unwrap();
        if let Some(gate) = gate {
            assert!(history.is_file());
            drop(gate);
            crate::service::snapshot::maintenance::retry_pending_deletions(&paths)
                .await
                .unwrap();
        }
        assert!(!history.exists());
        assert!(!paths
            .project_sessions_dir(workspace.path())
            .join(id)
            .exists());
    }
}

#[cfg(feature = "tools-image-analysis")]
#[tokio::test]
async fn delete_session_removes_its_persisted_tool_image_snapshot() {
    use crate::agentic::image_analysis::attachments;
    use crate::agentic::tools::{implementations::ViewImageTool, Tool, ToolResult, ToolUseContext};
    use crate::agentic::workspace::WorkspaceBinding;

    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let runtime = paths.project_runtime_root(workspace.path());
    let persistence = Arc::new(PersistenceManager::new(paths).unwrap());
    let manager = test_manager(persistence.clone());
    let session = manager
        .create_session(
            "Screenshot session".into(),
            "agent".into(),
            SessionConfig {
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let binding = WorkspaceBinding::new(
        Some(workspace.workspace_id().into()),
        workspace.path().into(),
    );
    let mut context = ToolUseContext::for_tool_listing(Some(binding), None);
    context.custom_data.insert(
        "__openbitfun_test_runtime_root".into(),
        serde_json::json!(runtime),
    );
    context.primary_model_facts =
        tool_runtime::context::PrimaryModelFacts::new("test", "vision", "openai", true);
    context.session_id = Some(session.session_id.clone());

    let original = workspace.path().join("original.png");
    let image = attachments::test_image();
    let (bytes, _) =
        crate::agentic::image_analysis::decode_data_url(image.data_url.as_deref().unwrap())
            .unwrap();
    tokio::fs::write(&original, &bytes).await.unwrap();
    let results = ViewImageTool::new()
        .call_impl(&serde_json::json!({"path":original}), &context)
        .await
        .unwrap();
    let ToolResult::Result {
        data,
        image_attachments,
        ..
    } = &results[0]
    else {
        panic!("Expected a tool image");
    };
    let reference = data["source_image_reference"].as_str().unwrap();
    assert!(reference.starts_with("openbitfun://current-session/attachments/images/"));
    let snapshot =
        std::path::PathBuf::from(context.resolve_tool_path(reference).unwrap().resolved_path);
    let mut result = Message::tool_result(crate::agentic::core::ToolResult {
        tool_id: "capture".into(),
        tool_name: "view_image".into(),
        effective_tool_name: None,
        result: data.clone(),
        result_for_assistant: None,
        is_error: false,
        duration_ms: None,
        image_attachments: image_attachments.clone(),
    });
    result.metadata.tool_image_references =
        attachments::tool_image_references(image_attachments.as_deref().unwrap(), &context)
            .await
            .unwrap();
    persistence
        .save_turn_context_snapshot(workspace.path(), &session.session_id, 0, &[result])
        .await
        .unwrap();
    assert_eq!(tokio::fs::read(&snapshot).await.unwrap(), bytes);
    let restored = persistence
        .load_turn_context_snapshot(workspace.path(), &session.session_id, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored[0].metadata.tool_image_references, [reference]);
    manager
        .delete_session(workspace.path(), &session.session_id)
        .await
        .unwrap();
    assert!(!snapshot.exists());
    assert_eq!(tokio::fs::read(original).await.unwrap(), bytes);
}
