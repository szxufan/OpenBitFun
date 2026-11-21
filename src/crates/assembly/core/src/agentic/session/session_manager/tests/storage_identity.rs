use super::super::CoreSessionStorePort;
use super::support::{test_manager, test_path_manager, TestWorkspace};
use crate::agentic::core::{Session, SessionConfig};
use crate::agentic::persistence::PersistenceManager;
#[cfg(feature = "remote-workspace")]
use crate::service::WorkspaceRuntimeService;
use openbitfun_runtime_ports::SessionStoragePathRequest;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn persistence_manager_accessor_reuses_runtime_owner() {
    let persistence_manager =
        Arc::new(PersistenceManager::new(test_path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());

    assert!(Arc::ptr_eq(
        &persistence_manager,
        &manager.persistence_manager()
    ));
}

#[tokio::test]
async fn core_session_store_port_resolves_local_storage_to_sessions_dir() {
    use openbitfun_runtime_ports::{
        SessionStorageKind, SessionStoragePathRequest, SessionStorePort,
    };

    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let port = CoreSessionStorePort::with_path_manager_for_tests(path_manager.clone());
    let resolution = port
        .resolve_session_storage_path(SessionStoragePathRequest {
            workspace_path: workspace.path().to_path_buf(),
            remote_connection_id: None,
            remote_ssh_host: None,
        })
        .await
        .expect("storage path should resolve");

    assert_eq!(resolution.storage_kind, SessionStorageKind::Local);
    assert_eq!(
        resolution.effective_storage_path,
        path_manager.project_sessions_dir(workspace.path())
    );
    assert_ne!(resolution.effective_storage_path, workspace.path());

    let resolved_again = port
        .resolve_session_storage_path(SessionStoragePathRequest {
            workspace_path: resolution.effective_storage_path.clone(),
            remote_connection_id: None,
            remote_ssh_host: None,
        })
        .await
        .expect("resolved sessions dir should pass through");
    assert_eq!(
        resolved_again.effective_storage_path,
        resolution.effective_storage_path
    );
}

#[cfg(feature = "remote-workspace")]
#[tokio::test]
async fn core_session_store_port_keeps_colliding_local_and_ssh_owners_separate() {
    use crate::service::workspace::legacy_compat::register_remote_fixture;
    use openbitfun_runtime_ports::{
        SessionStorageKind, SessionStoragePathRequest, SessionStorePort,
    };

    let workspace = TestWorkspace::new();
    let root = workspace.path().to_string_lossy().into_owned();
    let loopback = register_remote_fixture(&root, "connection-loopback", "localhost").await;
    let remote = register_remote_fixture(&root, "connection-remote", "other-host").await;
    let port = CoreSessionStorePort::with_path_manager_for_tests(workspace.path_manager());
    let local = port
        .resolve_workspace_storage(workspace.workspace_id())
        .await
        .unwrap();
    let loopback_storage = port.resolve_workspace_storage(&loopback.id).await.unwrap();
    let remote_storage = port.resolve_workspace_storage(&remote.id).await.unwrap();
    assert_eq!(local.storage_kind, SessionStorageKind::Local);
    assert_eq!(loopback_storage.storage_kind, SessionStorageKind::Remote);
    assert_ne!(
        local.effective_storage_path,
        loopback_storage.effective_storage_path
    );
    assert_ne!(
        loopback_storage.effective_storage_path,
        remote_storage.effective_storage_path
    );

    let old_request = SessionStoragePathRequest {
        workspace_path: workspace.path().to_path_buf(),
        remote_connection_id: None,
        remote_ssh_host: None,
    };
    let error = port
        .resolve_session_storage_path(old_request)
        .await
        .unwrap_err();
    assert!(error.message.contains("ambiguous"), "{error}");
    let selected = port
        .resolve_session_storage_path(SessionStoragePathRequest {
            workspace_path: workspace.path().to_path_buf(),
            remote_connection_id: Some("connection-loopback".into()),
            remote_ssh_host: Some("localhost".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        selected.effective_storage_path,
        loopback_storage.effective_storage_path
    );
    let unknown = port
        .resolve_storage_for_reference(Some("unknown-id"), &root, None, None)
        .await
        .unwrap_err();
    assert!(unknown.message.contains("unknown-id"));
}

#[tokio::test]
async fn core_session_store_port_legacy_worktree_uses_its_registered_project_storage() {
    use openbitfun_runtime_ports::{SessionStoragePathRequest, SessionStorePort};
    let project = TestWorkspace::new();
    let execution = project.path().join("worktree");
    std::fs::create_dir_all(&execution).unwrap();
    let record = crate::service::workspace::legacy_compat::register_local_fixture(
        &execution,
        Some(project.path()),
    )
    .await;
    let port = CoreSessionStorePort::with_path_manager_for_tests(project.path_manager());
    let by_id = port.resolve_workspace_storage(&record.id).await.unwrap();
    let by_legacy = port
        .resolve_session_storage_path(SessionStoragePathRequest {
            workspace_path: execution.clone(),
            remote_connection_id: None,
            remote_ssh_host: None,
        })
        .await
        .unwrap();
    assert_eq!(
        by_id.effective_storage_path,
        project.path_manager().project_sessions_dir(project.path())
    );
    assert_eq!(
        by_legacy.effective_storage_path,
        by_id.effective_storage_path
    );
    assert_eq!(by_legacy.requested_workspace_path, execution);

    std::fs::remove_dir_all(&execution).unwrap();
    let offline = port
        .resolve_session_storage_path(SessionStoragePathRequest {
            workspace_path: execution,
            remote_connection_id: None,
            remote_ssh_host: None,
        })
        .await
        .unwrap();
    assert_eq!(offline.effective_storage_path, by_id.effective_storage_path);
}

#[cfg(feature = "remote-workspace")]
#[tokio::test]
async fn bound_session_writes_and_deletion_do_not_reinfer_a_colliding_legacy_path() {
    let workspace = TestWorkspace::new();
    let persistence = Arc::new(PersistenceManager::new(workspace.path_manager()).unwrap());
    let manager = test_manager(persistence.clone());
    let session = manager
        .create_session(
            "Bound writes".into(),
            "Standard".into(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_id: Some(workspace.workspace_id().into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let storage = manager
        .require_session_storage_path(&session.session_id)
        .await
        .unwrap();
    crate::service::workspace::legacy_compat::register_remote_fixture(
        &workspace.path().to_string_lossy(),
        "colliding-remote",
        "localhost",
    )
    .await;
    manager
        .sessions
        .get_mut(&session.session_id)
        .unwrap()
        .config
        .workspace_id = None;

    let turn = manager
        .append_completed_local_command_turn(
            &session.session_id,
            "Bound local command".into(),
            Some("bound-local-turn".into()),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        persistence
            .load_dialog_turn(&storage, &session.session_id, 0)
            .await
            .unwrap()
            .unwrap()
            .turn_id,
        turn.turn_id
    );
    manager
        .delete_session_by_id(&session.session_id)
        .await
        .unwrap();
    assert!(persistence
        .load_dialog_turn(&storage, &session.session_id, 0)
        .await
        .unwrap()
        .is_none());
}

#[cfg(feature = "remote-workspace")]
#[tokio::test]
async fn session_storage_binding_survives_path_collision_eviction_and_restart() {
    let workspace = TestWorkspace::new();
    let persistence = Arc::new(PersistenceManager::new(workspace.path_manager()).unwrap());
    let manager = test_manager(persistence.clone());
    let session = manager
        .create_session(
            "Bound history".into(),
            "Standard".into(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_id: Some(workspace.workspace_id().into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let storage = manager
        .require_session_storage_path(&session.session_id)
        .await
        .unwrap();
    crate::service::workspace::legacy_compat::register_remote_fixture(
        &workspace.path().to_string_lossy(),
        "colliding-remote",
        "localhost",
    )
    .await;
    // In-memory legacy projections cannot replace an admitted storage owner.
    manager
        .sessions
        .get_mut(&session.session_id)
        .unwrap()
        .config
        .workspace_id = None;
    assert_eq!(
        manager
            .require_session_storage_path(&session.session_id)
            .await
            .unwrap(),
        storage
    );
    manager.evict_loaded_session_for_test(&session.session_id);
    assert_eq!(
        manager
            .require_session_storage_path(&session.session_id)
            .await
            .unwrap(),
        storage
    );
    let restored = manager
        .restore_session_from_storage_path(&storage, &session.session_id)
        .await
        .unwrap();
    assert_eq!(
        restored.config.workspace_id.as_deref(),
        Some(workspace.workspace_id())
    );
    manager.evict_loaded_session_for_test(&session.session_id);

    let restarted = test_manager(persistence);
    let restored = restarted
        .restore_session_from_storage_path(&storage, &session.session_id)
        .await
        .unwrap();
    assert_eq!(
        restored.config.workspace_id.as_deref(),
        Some(workspace.workspace_id())
    );
    assert_eq!(
        restarted
            .require_session_storage_path(&session.session_id)
            .await
            .unwrap(),
        storage
    );
}

#[cfg(feature = "remote-workspace")]
#[tokio::test]
async fn core_session_store_port_resolves_unresolved_remote_storage_path() {
    use openbitfun_runtime_ports::{
        PortErrorKind, SessionStorageKind, SessionStoragePathRequest, SessionStorePort,
    };

    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let port = CoreSessionStorePort::with_path_manager_for_tests(path_manager.clone());

    // A connection ID alone is transport metadata, not a workspace
    // identity. Without a registered remote record the request must fail
    // loudly instead of inventing an `_unresolved` mirror.
    let error = port
        .resolve_session_storage_path(SessionStoragePathRequest {
            workspace_path: PathBuf::from("/remote/project"),
            remote_connection_id: Some("conn-1".to_string()),
            remote_ssh_host: None,
        })
        .await
        .expect_err("unregistered remote reference must not resolve storage");
    assert_eq!(error.kind, PortErrorKind::InvalidRequest);
    assert!(
        error.message.contains("does not resolve"),
        "unexpected error: {}",
        error.message
    );

    // Sessions already persisted under a legacy `_unresolved` mirror stay
    // readable: the resolved sessions dir passes through with its kind.
    let legacy_unresolved_dir =
        openbitfun_services_core::workspace_identity::unresolved_remote_session_storage_dir(
            path_manager.remote_ssh_mirror_root_dir(),
            "conn-1",
            "/remote/project",
        );
    let resolution = port
        .resolve_session_storage_path(SessionStoragePathRequest {
            workspace_path: legacy_unresolved_dir.clone(),
            remote_connection_id: Some("conn-1".to_string()),
            remote_ssh_host: None,
        })
        .await
        .expect("legacy unresolved sessions dir should pass through");
    assert_eq!(
        resolution.storage_kind,
        SessionStorageKind::UnresolvedRemote
    );
    assert!(resolution.is_remote_storage());
    assert_eq!(resolution.remote_connection_id.as_deref(), Some("conn-1"));
    assert_eq!(resolution.effective_storage_path, legacy_unresolved_dir);
}

#[cfg(feature = "remote-workspace")]
#[tokio::test]
async fn core_session_store_port_resolved_remote_sessions_dir_passes_through_only_sessions_root() {
    use openbitfun_runtime_ports::{
        SessionStorageKind, SessionStoragePathRequest, SessionStorePort,
    };

    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let port = CoreSessionStorePort::with_path_manager_for_tests(path_manager.clone());
    WorkspaceRuntimeService::new(path_manager.clone())
        .ensure_remote_workspace_runtime("example-host", "/root/repo")
        .await
        .expect("remote runtime should be ensured");
    let runtime = WorkspaceRuntimeService::new(path_manager.clone())
        .context_for_remote_workspace("example-host", "/root/repo");
    let sessions_dir = runtime.sessions_dir;
    let resolved = port
        .resolve_session_storage_path(SessionStoragePathRequest {
            workspace_path: sessions_dir.clone(),
            remote_connection_id: None,
            remote_ssh_host: None,
        })
        .await
        .expect("resolved remote sessions dir should pass through");

    assert_eq!(resolved.storage_kind, SessionStorageKind::Remote);
    assert_eq!(resolved.effective_storage_path, sessions_dir);

    let runtime_root = runtime.runtime_root;
    let runtime_root_resolution = port
        .resolve_session_storage_path(SessionStoragePathRequest {
            workspace_path: runtime_root.clone(),
            remote_connection_id: None,
            remote_ssh_host: None,
        })
        .await;

    assert!(
        runtime_root_resolution.is_err(),
        "remote runtime root must not pass as a resolved sessions dir"
    );
}

#[tokio::test]
async fn restore_session_from_storage_path_accepts_resolved_sessions_dir() {
    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let persistence_manager =
        Arc::new(PersistenceManager::new(path_manager.clone()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let sessions_dir = path_manager.project_sessions_dir(workspace.path());
    let session_id = Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        session_id.clone(),
        "Resolved sessions restore".to_string(),
        "Standard".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().to_string()),
            ..Default::default()
        },
    );

    persistence_manager
        .save_session(&sessions_dir, &session)
        .await
        .expect("session should save to resolved sessions dir");

    let restored = manager
        .restore_session_from_storage_path(&sessions_dir, &session_id)
        .await
        .expect("storage restore should read the resolved sessions dir directly");

    assert_eq!(restored.session_id, session_id);
}

#[tokio::test]
async fn restore_session_workspace_api_does_not_accept_resolved_sessions_dir() {
    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let persistence_manager =
        Arc::new(PersistenceManager::new(path_manager.clone()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let sessions_dir = path_manager.project_sessions_dir(workspace.path());
    let session_id = Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        session_id.clone(),
        "Resolved sessions restore".to_string(),
        "Standard".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().to_string()),
            ..Default::default()
        },
    );

    persistence_manager
        .save_session(&sessions_dir, &session)
        .await
        .expect("session should save to resolved sessions dir");

    let result = manager.restore_session(&sessions_dir, &session_id).await;

    assert!(
        result.is_err(),
        "workspace restore should not accept an already-resolved sessions dir"
    );
}

#[tokio::test]
async fn restore_session_for_workspace_uses_remote_identity() {
    crate::service::workspace::legacy_compat::register_remote_fixture(
        "/home/wsp/project",
        "ssh-1",
        "dev-host",
    )
    .await;
    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let persistence_manager =
        Arc::new(PersistenceManager::new(path_manager.clone()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let sessions_dir = crate::service::WorkspaceRuntimeService::new(path_manager.clone())
        .context_for_remote_workspace("dev-host", "/home/wsp/project")
        .sessions_dir;
    let session_id = Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        session_id.clone(),
        "Remote identity restore".to_string(),
        "Standard".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some("/home/wsp/project".to_string()),
            remote_connection_id: Some("ssh-1".to_string()),
            remote_ssh_host: Some("dev-host".to_string()),
            ..Default::default()
        },
    );

    persistence_manager
        .save_session(&sessions_dir, &session)
        .await
        .expect("session should save to remote sessions dir");

    let restored = manager
        .restore_session_for_workspace(
            SessionStoragePathRequest {
                workspace_path: PathBuf::from("/home/wsp/project"),
                remote_connection_id: Some("ssh-1".to_string()),
                remote_ssh_host: Some("dev-host".to_string()),
            },
            &session_id,
        )
        .await
        .expect("workspace restore should use remote identity");

    assert_eq!(restored.session_id, session_id);
}
