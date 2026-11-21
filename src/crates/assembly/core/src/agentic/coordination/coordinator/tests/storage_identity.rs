use super::super::test_support::{test_coordinator, test_persistent_coordinator};
use super::super::{
    loaded_session_workspace_id_for_locator, session_storage_workspace_locator,
    CoreSessionStorePort,
};
use crate::agentic::core::SessionConfig;
use openbitfun_runtime_ports::{AgentThreadGoalGetRequest, AgentThreadGoalManagementPort};
use std::path::Path;
use std::path::PathBuf;

#[test]
fn worktree_execution_root_is_a_legacy_alias_for_project_storage() {
    assert_eq!(
        session_storage_workspace_locator(
            Some(r"D:\worktrees\session-1"),
            Some("D:/worktrees/session-1"),
            Some("D:/projects/OpenBitFun"),
        )
        .as_deref(),
        Some("D:/projects/OpenBitFun")
    );
}

#[test]
fn omitted_locator_reuses_the_loaded_session_storage_binding() {
    assert_eq!(
        session_storage_workspace_locator(
            None,
            Some("/worktrees/session-1"),
            Some("/projects/OpenBitFun"),
        )
        .as_deref(),
        None
    );
}

#[test]
fn unrelated_workspace_is_not_rewritten_to_the_project_storage_root() {
    assert_eq!(
        session_storage_workspace_locator(
            Some("/projects/other"),
            Some("/worktrees/session-1"),
            Some("/projects/OpenBitFun"),
        )
        .as_deref(),
        Some("/projects/other")
    );
}

/// Registers a remote SSH record whose root string equals `root`, the
/// shape from issue #3275 (inactive SSH workspace sharing a local path).
async fn register_same_path_remote(root: &Path) -> crate::service::workspace::WorkspaceInfo {
    // Workspace records store canonical local roots; mirror that string.
    let root = dunce::canonicalize(root).expect("canonical root");
    crate::service::workspace::legacy_compat::register_remote_fixture(
        &root.to_string_lossy(),
        &format!("ssh-collision-{}@remote.example", uuid::Uuid::new_v4()),
        "remote.example",
    )
    .await
}

#[tokio::test]
async fn loaded_local_session_resolves_storage_by_id_despite_same_path_remote_record() {
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, session_manager) = test_persistent_coordinator();
    let session = session_manager
        .create_session(
            "Local collision".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("create local session before the SSH record exists");
    let local_storage = session_manager
        .require_session_storage_path(&session.session_id)
        .await
        .expect("local storage");
    let remote = register_same_path_remote(workspace.path()).await;
    let locator = session
        .config
        .workspace_path
        .clone()
        .expect("normalized workspace path");

    // The legacy path adapter keeps refusing to guess between owners.
    let legacy = coordinator
        .resolve_session_restore_scope(&locator, None, Some("localhost"))
        .await
        .expect_err("bare path must stay ambiguous at the legacy boundary");
    assert!(legacy.to_string().contains("ambiguous"), "{legacy}");

    // A send for the loaded local session resolves through its workspace ID.
    let resolution = coordinator
        .resolve_loaded_session_restore_scope(&session.config, &locator, None, Some("localhost"))
        .await
        .expect("loaded local session must resolve by workspace ID");
    assert_eq!(resolution.effective_storage_path, local_storage);
    assert!(!resolution.is_remote_storage());
    assert_eq!(resolution.requested_workspace_path, PathBuf::from(&locator));
    session_manager
        .ensure_session_storage_path(&session.session_id, &resolution.effective_storage_path)
        .expect("storage binding must stay on the local owner");

    // A loaded-session thread goal read carries the same path-only locator.
    let goal = AgentThreadGoalManagementPort::get_thread_goal(
        &coordinator,
        AgentThreadGoalGetRequest {
            session_id: session.session_id.clone(),
            workspace_path: locator.clone(),
            remote_connection_id: None,
            remote_ssh_host: Some("localhost".to_string()),
        },
    )
    .await
    .expect("thread goal read must resolve the loaded session by workspace ID");
    assert_eq!(goal, None);

    // A request that explicitly selects the SSH connection does not adopt
    // the local binding; it resolves the remote owner and fails the binding check.
    let remote_connection = remote.remote_ssh_connection_id().map(ToOwned::to_owned);
    let explicit_remote = coordinator
        .resolve_loaded_session_restore_scope(
            &session.config,
            &locator,
            remote_connection.as_deref(),
            Some("remote.example"),
        )
        .await
        .expect("explicit SSH selector resolves the remote record");
    assert!(explicit_remote.is_remote_storage());
    assert_ne!(explicit_remote.effective_storage_path, local_storage);
    assert!(session_manager
        .validate_session_storage_path_binding(
            &session.session_id,
            &explicit_remote.effective_storage_path,
        )
        .is_err());

    // Unloaded then restored: the restored config keeps its workspace ID,
    // so the next send still resolves to the local owner.
    assert!(session_manager
        .unload_session_from_memory(&session.session_id)
        .await
        .expect("unload session"));
    let restored = coordinator
        .restore_session_from_storage_path(&local_storage, &session.session_id)
        .await
        .expect("restore local session by its storage binding");
    assert_eq!(restored.config.workspace_id, session.config.workspace_id);
    let restored_resolution = coordinator
        .resolve_loaded_session_restore_scope(&restored.config, &locator, None, None)
        .await
        .expect("restored local session must resolve by workspace ID");
    assert_eq!(restored_resolution.effective_storage_path, local_storage);

    session_manager
        .delete_session_by_id(&session.session_id)
        .await
        .expect("clean up persisted test session");
}

#[tokio::test]
async fn loaded_remote_session_resolves_its_remote_record_despite_same_path_local_record() {
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let local =
        crate::service::workspace::legacy_compat::register_local_fixture(workspace.path(), None)
            .await;
    let remote = register_same_path_remote(workspace.path()).await;
    let (coordinator, _) = test_coordinator();
    let locator = workspace.path().to_string_lossy().into_owned();
    let remote_config = SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_id: Some(remote.id.clone()),
        workspace_path: Some(locator.clone()),
        remote_connection_id: remote.remote_ssh_connection_id().map(ToOwned::to_owned),
        remote_ssh_host: Some("remote.example".to_string()),
        ..Default::default()
    };
    let resolution = coordinator
        .resolve_loaded_session_restore_scope(&remote_config, &locator, None, None)
        .await
        .expect("remote session resolves by its workspace ID");
    assert!(resolution.is_remote_storage());
    assert_eq!(
        resolution.remote_connection_id.as_deref(),
        remote.remote_ssh_connection_id()
    );
    let local_storage =
        CoreSessionStorePort::with_path_manager(coordinator.session_manager.path_manager())
            .resolve_storage_for_reference(Some(&local.id), &locator, None, None)
            .await
            .expect("local storage");
    assert_ne!(
        resolution.effective_storage_path,
        local_storage.effective_storage_path
    );

    // A locator that names some other root, or an explicit different SSH
    // connection, keeps the legacy path behavior instead of silently
    // adopting the loaded session's binding.
    let other = tempfile::tempdir().expect("other root");
    assert!(loaded_session_workspace_id_for_locator(
        &remote_config,
        &other.path().to_string_lossy(),
        None,
        None,
    )
    .is_none());
    assert!(loaded_session_workspace_id_for_locator(
        &remote_config,
        &locator,
        Some("another-connection"),
        None,
    )
    .is_none());
    assert!(loaded_session_workspace_id_for_locator(
        &remote_config,
        &locator,
        None,
        Some("another.example"),
    )
    .is_none());
    assert_eq!(
        loaded_session_workspace_id_for_locator(
            &remote_config,
            &locator,
            None,
            Some("remote.example"),
        ),
        Some(remote.id.as_str())
    );
}

#[tokio::test]
async fn loaded_worktree_session_resolves_project_storage_despite_same_path_remote_record() {
    let project_dir = tempfile::tempdir().expect("project");
    let project = dunce::canonicalize(project_dir.path()).expect("canonical project");
    let worktree = project.join("worktree");
    std::fs::create_dir_all(&worktree).expect("worktree dir");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(project.as_path());
    let record = crate::service::workspace::legacy_compat::register_local_fixture(
        &worktree,
        Some(project.as_path()),
    )
    .await;
    register_same_path_remote(&worktree).await;
    register_same_path_remote(project.as_path()).await;
    let (coordinator, _) = test_coordinator();
    let port = CoreSessionStorePort::with_path_manager(coordinator.session_manager.path_manager());
    let expected = port
        .resolve_storage_for_reference(Some(&record.id), "", None, None)
        .await
        .expect("worktree storage by ID");
    let config = SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_id: Some(record.id.clone()),
        workspace_path: Some(worktree.to_string_lossy().into_owned()),
        project_workspace_path: Some(project.as_path().to_string_lossy().into_owned()),
        ..Default::default()
    };
    for locator in [
        worktree.to_string_lossy().into_owned(),
        project.as_path().to_string_lossy().into_owned(),
    ] {
        let legacy = coordinator
            .resolve_session_restore_scope(&locator, None, None)
            .await
            .expect_err("bare path must stay ambiguous at the legacy boundary");
        assert!(legacy.to_string().contains("ambiguous"), "{legacy}");
        let resolution = coordinator
            .resolve_loaded_session_restore_scope(&config, &locator, None, None)
            .await
            .expect("worktree session resolves by workspace ID");
        assert_eq!(
            resolution.effective_storage_path,
            expected.effective_storage_path
        );
        assert!(!resolution.is_remote_storage());
    }
}

#[tokio::test]
async fn loaded_worktree_session_with_missing_record_retries_project_id_only() {
    let project_dir = tempfile::tempdir().expect("project");
    let project = dunce::canonicalize(project_dir.path()).expect("canonical project");
    let worktree = project.join("removed-worktree");
    std::fs::create_dir_all(&worktree).expect("worktree dir");
    let project_record =
        crate::service::workspace::legacy_compat::register_local_fixture(&project, None).await;
    register_same_path_remote(&project).await;
    let (coordinator, _) = test_coordinator();
    let port = CoreSessionStorePort::with_path_manager(coordinator.session_manager.path_manager());
    let project_storage = port
        .resolve_storage_for_reference(Some(&project_record.id), "", None, None)
        .await
        .expect("project storage by ID");
    let missing_worktree_id = format!("removed-worktree-{}", uuid::Uuid::new_v4());
    let config = SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_id: Some(missing_worktree_id.clone()),
        project_workspace_id: Some(project_record.id.clone()),
        workspace_path: Some(worktree.to_string_lossy().into_owned()),
        project_workspace_path: Some(project.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let resolution = coordinator
        .resolve_loaded_session_restore_scope(&config, &project.to_string_lossy(), None, None)
        .await
        .expect("a removed worktree record retries the project workspace ID");
    assert_eq!(
        resolution.effective_storage_path,
        project_storage.effective_storage_path
    );
    assert!(!resolution.is_remote_storage());

    // Without a project ID there is nothing ID-based to retry: fail loudly.
    let without_project = SessionConfig {
        project_workspace_id: None,
        ..config.clone()
    };
    let error = coordinator
        .resolve_loaded_session_restore_scope(
            &without_project,
            &project.to_string_lossy(),
            None,
            None,
        )
        .await
        .expect_err("a missing workspace record must not fall back to a path");
    assert!(error.to_string().contains(&missing_worktree_id), "{error}");

    // Remote sessions never retry through another record.
    let remote = SessionConfig {
        remote_connection_id: Some("ssh-missing".to_string()),
        remote_ssh_host: Some("remote.example".to_string()),
        workspace_kind: Some(openbitfun_core_types::WorkspaceKind::Remote),
        ..config
    };
    assert!(coordinator
        .resolve_loaded_session_restore_scope(
            &remote,
            &project.to_string_lossy(),
            Some("ssh-missing"),
            None,
        )
        .await
        .is_err());
}

#[test]
fn loaded_remote_session_locator_uses_posix_comparison() {
    let config = SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_id: Some("remote-id".to_string()),
        workspace_path: Some("/srv/Project".to_string()),
        remote_connection_id: Some("ssh-a".to_string()),
        remote_ssh_host: Some("remote.example".to_string()),
        workspace_kind: Some(openbitfun_core_types::WorkspaceKind::Remote),
        ..Default::default()
    };
    assert_eq!(
        loaded_session_workspace_id_for_locator(&config, "/srv/Project/", None, None),
        Some("remote-id")
    );
    // POSIX remote roots are case-sensitive on every controller OS.
    assert_eq!(
        loaded_session_workspace_id_for_locator(&config, "/srv/project", None, None),
        None
    );
}
