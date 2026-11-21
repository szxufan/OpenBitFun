use super::super::test_support::test_coordinator_with_config_and_ownership;
use super::super::ConversationCoordinator;
use crate::agentic::core::SessionConfig;
use crate::infrastructure::PathManager;
use crate::runtime_ownership::CoreRuntimeOwnership;
#[cfg(feature = "remote-workspace")]
use crate::service::workspace::WorkspaceKind;
#[cfg(not(feature = "remote-workspace"))]
use crate::util::errors::OpenBitFunError;
use openbitfun_runtime_ports::SessionStoragePathRequest;
#[cfg(feature = "remote-workspace")]
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::test]
async fn remote_session_metadata_never_falls_back_to_a_local_workspace_binding() {
    // A remote binding is built from the registered SSH workspace record;
    // register it here instead of depending on another test's fixture.
    let registered_root = format!("/srv/remote-project-{}", uuid::Uuid::new_v4());
    let unregistered_root = format!("/srv/unregistered-project-{}", uuid::Uuid::new_v4());
    crate::service::workspace::legacy_compat::register_remote_fixture(
        &registered_root,
        "ssh-user@example.test:22",
        "example.test",
    )
    .await;
    let binding = ConversationCoordinator::build_workspace_binding(&SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_path: Some(registered_root.clone()),
        remote_connection_id: Some("ssh-user@example.test:22".to_string()),
        remote_ssh_host: Some("example.test".to_string()),
        ..SessionConfig::default()
    })
    .await
    .expect("remote metadata must remain a remote binding");

    assert!(binding.is_remote());
    assert_eq!(binding.connection_id(), Some("ssh-user@example.test:22"));
    assert!(
        ConversationCoordinator::build_workspace_services(&Some(binding))
            .await
            .is_err(),
        "a remote binding without a reachable provider must be an error, never local services"
    );

    // Partial remote metadata for a root no workspace record owns must
    // fail closed instead of becoming a local binding.
    for incomplete in [
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(unregistered_root.clone()),
            remote_connection_id: Some("ssh-user@example.test:22".to_string()),
            ..SessionConfig::default()
        },
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(unregistered_root.clone()),
            remote_ssh_host: Some("example.test".to_string()),
            ..SessionConfig::default()
        },
    ] {
        assert!(
            ConversationCoordinator::build_workspace_binding(&incomplete)
                .await
                .is_none(),
            "incomplete remote metadata must fail closed instead of becoming local"
        );
    }
}

#[tokio::test]
async fn remote_workspace_services_unavailable_is_an_error() {
    crate::service::workspace::legacy_compat::register_remote_fixture(
        "/srv/remote-project",
        "ssh-user@example.test:22",
        "example.test",
    )
    .await;
    let binding = ConversationCoordinator::build_workspace_binding(&SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_path: Some("/srv/remote-project".to_string()),
        remote_connection_id: Some("ssh-user@example.test:22".to_string()),
        remote_ssh_host: Some("example.test".to_string()),
        ..SessionConfig::default()
    })
    .await
    .expect("remote metadata must remain a remote binding");

    // No remote workspace manager is initialized in this test binary, so
    // the provider cannot be built. The turn must fail with a typed error
    // that names the connection instead of running the session against
    // the controller filesystem.
    let error = ConversationCoordinator::build_workspace_services(&Some(binding))
        .await
        .expect_err("an unserved remote binding must not yield local services");
    let message = error.to_string();
    assert!(
        message.contains("ssh-user@example.test:22"),
        "error must name the connection: {message}"
    );
    assert!(
        message.contains("no local fallback was attempted"),
        "error must state that no fallback happened: {message}"
    );
    #[cfg(not(feature = "remote-workspace"))]
    assert!(
        matches!(error, OpenBitFunError::NotImplemented(_)),
        "builds without remote-workspace must report the missing feature: {message}"
    );

    let local = ConversationCoordinator::build_workspace_services(&None)
        .await
        .expect("no binding is not an error");
    assert!(local.is_none());
}

#[tokio::test]
async fn create_session_checks_runtime_ownership_before_persisting() {
    let ownership_root = tempfile::tempdir().expect("ownership root");
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let key = openbitfun_services_core::runtime_ownership::RuntimeOwnershipKey::for_workspace(
        workspace.path(),
        "openbitfun",
    )
    .expect("ownership key");
    let _shared =
        openbitfun_services_core::runtime_ownership::WorkspaceRuntimeOwnership::try_acquire(
            ownership_root.path(),
            &key,
            openbitfun_services_core::runtime_ownership::RuntimeDeployment::Shared,
        )
        .expect("shared owner");
    let owner = Arc::new(CoreRuntimeOwnership::embedded_with_facts(
        ownership_root.path().to_path_buf(),
        "openbitfun".to_string(),
        "test",
    ));
    let (coordinator, session_manager) =
        test_coordinator_with_config_and_ownership(100, true, owner);

    let error = coordinator
        .create_session_with_id(
            Some("ownership-conflict".to_string()),
            "blocked".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect_err("Shared owner must block local session creation");

    assert!(error.to_string().contains("ownership"));
    assert!(session_manager.get_session("ownership-conflict").is_none());
}

#[tokio::test]
async fn assistant_bootstrap_checks_runtime_ownership_before_files_or_attach() {
    let ownership_root = tempfile::tempdir().expect("ownership root");
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let key = openbitfun_services_core::runtime_ownership::RuntimeOwnershipKey::for_workspace(
        workspace.path(),
        "openbitfun",
    )
    .expect("ownership key");
    let _shared =
        openbitfun_services_core::runtime_ownership::WorkspaceRuntimeOwnership::try_acquire(
            ownership_root.path(),
            &key,
            openbitfun_services_core::runtime_ownership::RuntimeDeployment::Shared,
        )
        .expect("shared owner");
    let owner = Arc::new(CoreRuntimeOwnership::embedded_with_facts(
        ownership_root.path().to_path_buf(),
        "openbitfun".to_string(),
        "test",
    ));
    let (coordinator, session_manager) =
        test_coordinator_with_config_and_ownership(100, true, owner);

    let error = coordinator
        .ensure_assistant_bootstrap(
            "assistant-bootstrap-conflict".to_string(),
            workspace.path().to_string_lossy().to_string(),
        )
        .await
        .expect_err("Shared owner must block assistant bootstrap");

    assert!(error.to_string().contains("ownership"));
    assert!(session_manager
        .get_session("assistant-bootstrap-conflict")
        .is_none());
    assert_eq!(
        std::fs::read_dir(workspace.path())
            .expect("workspace remains readable")
            .count(),
        0,
        "ownership failure must happen before persona or gitignore writes"
    );
}

#[test]
fn workspace_open_owner_gates_before_open_and_guards_snapshot_by_kind() {
    let source = include_str!("../../coordinator.rs");
    let helper = source
        .split("pub async fn select_workspace_with_runtime_ownership")
        .nth(1)
        .and_then(|source| {
            source
                .split("pub async fn create_local_workspace_with_runtime_ownership")
                .next()
        })
        .expect("workspace open owner");
    let ownership_gate = helper
        .find("ensure_runtime_ownership")
        .expect("workspace ownership gate");
    let workspace_open = helper
        .find("open_workspace_by_id")
        .expect("workspace open call");
    assert!(ownership_gate < workspace_open);
    assert!(helper.contains("WorkspaceKind::Remote"));
    assert!(helper.contains("initialize_snapshot_manager_for_workspace"));

    let bot_router = include_str!("../../../../service/remote_connect/bot/command_router.rs");
    assert!(bot_router.contains("select_workspace_with_runtime_ownership"));
    assert!(!bot_router.contains("initialize_snapshot_manager_for_workspace"));
}

#[cfg(feature = "remote-workspace")]
#[tokio::test]
async fn workspace_open_owner_resolves_known_remote_before_ownership_gate() {
    let root = tempfile::tempdir().expect("test root");
    let path_manager = Arc::new(PathManager::with_user_root_for_tests(
        root.path().join("user-root"),
    ));
    let workspace_service =
        crate::service::workspace::WorkspaceService::new_for_test_path_manager(path_manager).await;
    let remote_path = PathBuf::from(format!(
        "/openbitfun-tests/known-remote-{}",
        uuid::Uuid::new_v4()
    ));
    workspace_service
        .track_workspace_activity(
            remote_path.clone(),
            crate::service::workspace::WorkspaceCreateOptions {
                workspace_kind: WorkspaceKind::Remote,
                remote_connection_id: Some("conn-known-remote".to_string()),
                remote_ssh_host: Some("known-host".to_string()),
                ..Default::default()
            },
            crate::service::workspace::WorkspaceActivityMode::RefreshMetadata,
        )
        .await
        .expect("remember remote workspace");
    let owner = Arc::new(CoreRuntimeOwnership::embedded_with_facts(
        root.path().join("ownership"),
        "openbitfun".to_string(),
        "test",
    ));
    let (coordinator, _) = test_coordinator_with_config_and_ownership(100, false, owner);

    let opened = coordinator
        .upgrade_legacy_workspace_with_runtime_ownership(
            &workspace_service,
            remote_path,
            None,
            None,
            "known remote test",
        )
        .await
        .expect("path-only known remote must not acquire a local lease");

    assert_eq!(opened.workspace_kind, WorkspaceKind::Remote);
    assert_eq!(opened.remote_ssh_connection_id(), Some("conn-known-remote"));
}

#[cfg(feature = "remote-workspace")]
#[tokio::test]
async fn remote_workspace_runtime_ownership_recovers_without_selecting_the_workspace() {
    let root = tempfile::tempdir().expect("test root");
    let path_manager = Arc::new(PathManager::with_user_root_for_tests(
        root.path().join("user"),
    ));
    let workspace_service =
        crate::service::workspace::WorkspaceService::new_for_test_path_manager(path_manager).await;
    let remote_path = PathBuf::from(format!("/remote/project-{}", uuid::Uuid::new_v4()));
    for (connection, host) in [("conn-a", "host-a"), ("conn-b", "host-b")] {
        workspace_service
            .track_workspace_activity(
                remote_path.clone(),
                crate::service::workspace::WorkspaceCreateOptions {
                    workspace_kind: WorkspaceKind::Remote,
                    remote_connection_id: Some(connection.to_string()),
                    remote_ssh_host: Some(host.to_string()),
                    ..Default::default()
                },
                crate::service::workspace::WorkspaceActivityMode::TouchOnly,
            )
            .await
            .expect("remember remote workspace");
    }
    let owner = Arc::new(CoreRuntimeOwnership::embedded_with_facts(
        root.path().join("ownership"),
        "openbitfun".to_string(),
        "test",
    ));
    let (coordinator, _) = test_coordinator_with_config_and_ownership(100, false, owner);
    assert!(coordinator
        .ensure_workspace_runtime_ownership(&remote_path, Some("conn-a"), Some("host-a"))
        .is_err());
    coordinator
        .ensure_workspace_runtime_ownership_for_reference_with_service(
            &workspace_service,
            None,
            &remote_path.to_string_lossy(),
            Some("conn-a"),
            Some("host-a"),
        )
        .await
        .expect("recover target A from the Workspace owner's saved facts");
    coordinator
        .ensure_workspace_runtime_ownership(&remote_path, Some("conn-a"), Some("host-a"))
        .expect("restored session mutations can run without a desktop workspace selection");
    assert!(coordinator
        .ensure_workspace_runtime_ownership(&remote_path, Some("conn-b"), Some("host-b"))
        .is_err());
    assert!(workspace_service.get_opened_workspaces().await.is_empty());
    assert!(workspace_service.get_current_workspace().await.is_none());
    assert!(!remote_path.exists());
}

#[cfg(feature = "remote-workspace")]
#[tokio::test]
async fn remote_workspace_runtime_ownership_rejects_mismatched_saved_identity() {
    let root = tempfile::tempdir().expect("test root");
    let path_manager = Arc::new(PathManager::with_user_root_for_tests(
        root.path().join("user"),
    ));
    let workspace_service =
        crate::service::workspace::WorkspaceService::new_for_test_path_manager(path_manager).await;
    let remote_path = root.path().join("same-path-local-sentinel");
    std::fs::create_dir(&remote_path).expect("local collision");
    std::fs::write(remote_path.join("owner.txt"), "local").expect("local sentinel");
    workspace_service
        .track_workspace_activity(
            remote_path.clone(),
            crate::service::workspace::WorkspaceCreateOptions {
                workspace_kind: WorkspaceKind::Remote,
                remote_connection_id: Some("conn-a".to_string()),
                remote_ssh_host: Some("host-a".to_string()),
                ..Default::default()
            },
            crate::service::workspace::WorkspaceActivityMode::TouchOnly,
        )
        .await
        .expect("remember remote workspace");
    let owner = Arc::new(CoreRuntimeOwnership::embedded_with_facts(
        root.path().join("ownership"),
        "openbitfun".to_string(),
        "test",
    ));
    let (coordinator, _) = test_coordinator_with_config_and_ownership(100, false, owner);
    for (connection, host) in [("unknown", "host-a"), ("conn-a", "wrong-host")] {
        let error = coordinator
            .ensure_workspace_runtime_ownership_for_reference_with_service(
                &workspace_service,
                None,
                &remote_path.to_string_lossy(),
                Some(connection),
                Some(host),
            )
            .await
            .expect_err("a matching path or one identity field must not authorize another target");
        assert!(error.to_string().contains("saved workspace does not match"));
    }
    assert!(coordinator
        .ensure_workspace_runtime_ownership(&remote_path, Some("conn-a"), Some("host-a"))
        .is_err());
    assert_eq!(
        std::fs::read_to_string(remote_path.join("owner.txt")).unwrap(),
        "local"
    );
    assert!(workspace_service.get_opened_workspaces().await.is_empty());
}

#[tokio::test]
async fn unverified_remote_hint_cannot_bypass_local_workspace_ownership() {
    let ownership_root = tempfile::tempdir().expect("ownership root");
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let key = openbitfun_services_core::runtime_ownership::RuntimeOwnershipKey::for_workspace(
        workspace.path(),
        "openbitfun",
    )
    .expect("ownership key");
    let _shared =
        openbitfun_services_core::runtime_ownership::WorkspaceRuntimeOwnership::try_acquire(
            ownership_root.path(),
            &key,
            openbitfun_services_core::runtime_ownership::RuntimeDeployment::Shared,
        )
        .expect("shared owner");
    let owner = Arc::new(CoreRuntimeOwnership::embedded_with_facts(
        ownership_root.path().to_path_buf(),
        "openbitfun".to_string(),
        "test",
    ));
    let (coordinator, _) = test_coordinator_with_config_and_ownership(100, false, owner);
    let path_manager = Arc::new(PathManager::with_user_root_for_tests(
        workspace.path().join("user-root"),
    ));
    let workspace_service =
        crate::service::workspace::WorkspaceService::new_for_test_path_manager(path_manager).await;

    let error = coordinator
        .upgrade_legacy_workspace_with_runtime_ownership(
            &workspace_service,
            workspace.path().to_path_buf(),
            Some("bogus-connection"),
            Some("bogus-host"),
            "unverified remote hint test",
        )
        .await
        .expect_err("unverified hints must not bypass local ownership");

    #[cfg(feature = "ssh-remote")]
    assert!(
        error.to_string().contains("not saved on this host"),
        "{error}"
    );
    #[cfg(not(feature = "ssh-remote"))]
    assert!(
        error.to_string().contains("requires SSH support"),
        "{error}"
    );
    assert!(workspace_service.get_opened_workspaces().await.is_empty());
}

#[tokio::test]
async fn attach_and_mutation_paths_check_runtime_ownership_before_side_effects() {
    let ownership_root = tempfile::tempdir().expect("ownership root");
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let key = openbitfun_services_core::runtime_ownership::RuntimeOwnershipKey::for_workspace(
        workspace.path(),
        "openbitfun",
    )
    .expect("ownership key");
    let _shared =
        openbitfun_services_core::runtime_ownership::WorkspaceRuntimeOwnership::try_acquire(
            ownership_root.path(),
            &key,
            openbitfun_services_core::runtime_ownership::RuntimeDeployment::Shared,
        )
        .expect("shared owner");
    let owner = Arc::new(CoreRuntimeOwnership::embedded_with_facts(
        ownership_root.path().to_path_buf(),
        "openbitfun".to_string(),
        "test",
    ));
    let (coordinator, session_manager) =
        test_coordinator_with_config_and_ownership(100, true, owner);
    let workspace_path = workspace.path().to_string_lossy().to_string();

    let hidden_error = coordinator
        .create_hidden_subagent_session_with_workspace(
            Some("hidden-ownership-conflict".to_string()),
            "hidden".to_string(),
            "Standard".to_string(),
            SessionConfig::default(),
            workspace_path.clone(),
            None,
        )
        .await
        .expect_err("Hidden session creation must honor runtime ownership");
    assert!(hidden_error.to_string().contains("ownership"));

    let restore_error = coordinator
        .restore_session_for_workspace(
            SessionStoragePathRequest {
                workspace_path: workspace.path().to_path_buf(),
                remote_connection_id: None,
                remote_ssh_host: None,
            },
            "missing-session",
        )
        .await
        .expect_err("Runtime attach must honor ownership before reading persistence");
    assert!(restore_error.to_string().contains("ownership"));

    let archive_error = openbitfun_runtime_ports::AgentSessionManagementPort::set_session_archived(
        &coordinator,
        openbitfun_runtime_ports::AgentSessionArchiveStateRequest {
            workspace_id: None,
            workspace_path,
            session_id: "missing-session".to_string(),
            archived: true,
            remote_connection_id: None,
            remote_ssh_host: None,
        },
    )
    .await
    .expect_err("Metadata mutation must honor ownership before touching persistence");
    assert!(archive_error.message.contains("ownership"));
    assert!(session_manager
        .get_session("hidden-ownership-conflict")
        .is_none());
}
