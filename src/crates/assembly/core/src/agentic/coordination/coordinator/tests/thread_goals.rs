use super::super::test_support::test_coordinator;
#[cfg(feature = "remote-workspace")]
use super::super::test_support::test_persistent_coordinator;
#[cfg(feature = "remote-workspace")]
use crate::agentic::core::SessionConfig;
#[cfg(feature = "remote-workspace")]
use crate::agentic::goal_mode::thread_goal_patch;
#[cfg(feature = "remote-workspace")]
use crate::service::session::SessionMetadata;
use openbitfun_runtime_ports::{AgentThreadGoalGetRequest, AgentThreadGoalManagementPort};
#[cfg(feature = "remote-workspace")]
use openbitfun_runtime_ports::{ThreadGoal, ThreadGoalStatus};

#[tokio::test]
async fn thread_goal_management_preserves_validation_error_messages() {
    let (coordinator, _) = test_coordinator();
    let error = AgentThreadGoalManagementPort::get_thread_goal(
        &coordinator,
        AgentThreadGoalGetRequest {
            session_id: "../invalid".to_string(),
            workspace_path: std::env::temp_dir().to_string_lossy().into_owned(),
            remote_connection_id: None,
            remote_ssh_host: None,
        },
    )
    .await
    .expect_err("invalid session id should be rejected");

    assert_eq!(
        error.kind,
        openbitfun_runtime_ports::PortErrorKind::InvalidRequest
    );
    assert!(error.message.starts_with("Validation error:"));
}

#[cfg(feature = "remote-workspace")]
#[tokio::test]
async fn thread_goal_management_keeps_cold_remote_workspaces_isolated() {
    let (coordinator, session_manager) = test_coordinator();
    let fixture_id = uuid::Uuid::new_v4();
    let session_id = format!("remote-goal-{fixture_id}");
    let logical_workspace_path = "/workspace/shared";
    let remote_identities = [
        (
            format!("connection-a-{fixture_id}"),
            format!("host-a-{fixture_id}"),
            "Goal from remote A",
        ),
        (
            format!("connection-b-{fixture_id}"),
            format!("host-b-{fixture_id}"),
            "Goal from remote B",
        ),
    ];
    let mut storage_paths = Vec::new();

    for (index, (connection_id, ssh_host, objective)) in remote_identities.iter().enumerate() {
        crate::service::workspace::legacy_compat::register_remote_fixture(
            logical_workspace_path,
            connection_id,
            ssh_host,
        )
        .await;
        let storage_path = coordinator
            .resolve_session_restore_scope(
                logical_workspace_path,
                Some(connection_id),
                Some(ssh_host),
            )
            .await
            .map(|resolution| resolution.effective_storage_path)
            .expect("remote storage path should resolve");
        let goal = ThreadGoal {
            goal_id: format!("goal-{index}"),
            session_id: session_id.clone(),
            objective: (*objective).to_string(),
            status: ThreadGoalStatus::Active,
            token_budget: None,
            tokens_used: 0,
            time_used_seconds: 0,
            created_at: index as i64,
            updated_at: index as i64,
            auto_continuation_count: 0,
        };
        let mut metadata = SessionMetadata::new(
            session_id.clone(),
            format!("Remote {index}"),
            "Standard".to_string(),
            "primary".to_string(),
        );
        metadata.custom_metadata = Some(thread_goal_patch(&goal));
        metadata.workspace_path = Some(logical_workspace_path.to_string());
        metadata.workspace_hostname = Some(ssh_host.clone());
        session_manager
            .persistence_manager()
            .save_session_metadata(&storage_path, &metadata)
            .await
            .expect("remote goal metadata should persist");
        storage_paths.push(storage_path);
    }

    assert!(session_manager.get_session(&session_id).is_none());

    for (connection_id, ssh_host, objective) in &remote_identities {
        let goal = AgentThreadGoalManagementPort::get_thread_goal(
            &coordinator,
            AgentThreadGoalGetRequest {
                session_id: session_id.clone(),
                workspace_path: logical_workspace_path.to_string(),
                remote_connection_id: Some(connection_id.clone()),
                remote_ssh_host: Some(ssh_host.clone()),
            },
        )
        .await
        .expect("cold remote goal lookup should succeed")
        .expect("cold remote goal should exist");
        assert_eq!(goal.objective, *objective);
    }

    let loaded_session_id = format!("loaded-remote-goal-{fixture_id}");
    coordinator
        .ensure_verified_remote_workspace_runtime_ownership(
            std::path::Path::new(logical_workspace_path),
            &remote_identities[0].0,
            Some(&remote_identities[0].1),
        )
        .expect("Workspace owner should verify the remote binding before loading a session");
    coordinator
        .create_session_with_id(
            Some(loaded_session_id.clone()),
            "Loaded remote A".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(logical_workspace_path.to_string()),
                remote_connection_id: Some(remote_identities[0].0.clone()),
                remote_ssh_host: Some(remote_identities[0].1.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("remote A should enter the loaded session set");
    let loaded_storage_path = session_manager
        .resolve_session_workspace_binding(&loaded_session_id)
        .await
        .expect("loaded remote binding should resolve")
        .session_storage_dir();
    assert_eq!(loaded_storage_path, storage_paths[0]);
    let loaded_goal_fixture = ThreadGoal {
        goal_id: "loaded-goal-a".to_string(),
        session_id: loaded_session_id.clone(),
        objective: "Loaded goal from remote A".to_string(),
        status: ThreadGoalStatus::Active,
        token_budget: None,
        tokens_used: 0,
        time_used_seconds: 0,
        created_at: 0,
        updated_at: 0,
        auto_continuation_count: 0,
    };
    let mut loaded_metadata = SessionMetadata::new(
        loaded_session_id.clone(),
        "Loaded remote A".to_string(),
        "Standard".to_string(),
        "primary".to_string(),
    );
    loaded_metadata.custom_metadata = Some(thread_goal_patch(&loaded_goal_fixture));
    loaded_metadata.workspace_path = Some(logical_workspace_path.to_string());
    loaded_metadata.workspace_hostname = Some(remote_identities[0].1.clone());
    session_manager
        .persistence_manager()
        .save_session_metadata(&loaded_storage_path, &loaded_metadata)
        .await
        .expect("loaded remote goal should persist");
    let loaded_goal = AgentThreadGoalManagementPort::get_thread_goal(
        &coordinator,
        AgentThreadGoalGetRequest {
            session_id: loaded_session_id.clone(),
            workspace_path: ".".to_string(),
            remote_connection_id: None,
            remote_ssh_host: None,
        },
    )
    .await
    .expect("loaded remote goal lookup with default workspace should succeed")
    .expect("loaded remote goal should exist");
    assert_eq!(loaded_goal.objective, "Loaded goal from remote A");

    let explicit_loaded_goal = AgentThreadGoalManagementPort::get_thread_goal(
        &coordinator,
        AgentThreadGoalGetRequest {
            session_id: loaded_session_id.clone(),
            workspace_path: logical_workspace_path.to_string(),
            remote_connection_id: Some(remote_identities[0].0.clone()),
            remote_ssh_host: Some(remote_identities[0].1.clone()),
        },
    )
    .await
    .expect("loaded remote goal lookup with matching identity should succeed")
    .expect("loaded remote goal should exist");
    assert_eq!(explicit_loaded_goal.objective, "Loaded goal from remote A");

    let cross_workspace_error = AgentThreadGoalManagementPort::get_thread_goal(
        &coordinator,
        AgentThreadGoalGetRequest {
            session_id: loaded_session_id,
            workspace_path: logical_workspace_path.to_string(),
            remote_connection_id: Some(remote_identities[1].0.clone()),
            remote_ssh_host: Some(remote_identities[1].1.clone()),
        },
    )
    .await
    .expect_err("a loaded session must not read another remote workspace");
    assert_eq!(
        cross_workspace_error.kind,
        openbitfun_runtime_ports::PortErrorKind::InvalidRequest
    );
    assert!(cross_workspace_error
        .message
        .contains("already bound to another workspace"));

    for storage_path in storage_paths {
        let _ = std::fs::remove_dir_all(storage_path);
    }
}

#[cfg(feature = "remote-workspace")]
#[tokio::test]
async fn thread_goal_mutations_use_loaded_remote_workspace_facts() {
    let (coordinator, session_manager) = test_persistent_coordinator();
    let fixture_id = uuid::Uuid::new_v4();
    let session_id = format!("remote-goal-mutation-{fixture_id}");
    let logical_workspace_path = format!("/workspace/remote-goal-{fixture_id}");
    let remote_connection_id = format!("connection-{fixture_id}");
    let remote_ssh_host = format!("host-{fixture_id}");
    crate::service::workspace::legacy_compat::register_remote_fixture(
        &logical_workspace_path,
        &remote_connection_id,
        &remote_ssh_host,
    )
    .await;

    coordinator
        .ensure_verified_remote_workspace_runtime_ownership(
            std::path::Path::new(&logical_workspace_path),
            &remote_connection_id,
            Some(&remote_ssh_host),
        )
        .expect("Workspace owner should verify the remote binding before loading a session");
    coordinator
        .create_session_with_id(
            Some(session_id.clone()),
            "Remote goal mutation".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(logical_workspace_path.clone()),
                remote_connection_id: Some(remote_connection_id),
                remote_ssh_host: Some(remote_ssh_host),
                ..Default::default()
            },
        )
        .await
        .expect("remote session should load without local ownership");

    let created = AgentThreadGoalManagementPort::create_thread_goal(
        &coordinator,
        openbitfun_runtime_ports::AgentThreadGoalCreateRequest {
            session_id: session_id.clone(),
            workspace_path: logical_workspace_path.clone(),
            objective: "Keep remote ownership structured".to_string(),
            token_budget: None,
        },
    )
    .await
    .expect("remote goal creation must not acquire a local workspace lock");
    let updated = AgentThreadGoalManagementPort::update_thread_goal_status(
        &coordinator,
        openbitfun_runtime_ports::AgentThreadGoalUpdateStatusRequest {
            session_id: session_id.clone(),
            workspace_path: logical_workspace_path,
            status: ThreadGoalStatus::Complete,
            turn_id: None,
        },
    )
    .await
    .expect("remote goal update must not acquire a local workspace lock");

    assert_eq!(created.session_id, session_id);
    assert_eq!(updated.status, ThreadGoalStatus::Complete);

    assert!(coordinator
        .prepare_prompt_thread_goal(&session_id, "/goal Repair remote login\nand verify")
        .await
        .expect("plain remote prompt must activate a goal")
        .is_some());
    let storage_path = coordinator
        .require_main_session_storage_path(&session_id)
        .await
        .unwrap();
    let activated = coordinator
        .thread_goal_store()
        .get_thread_goal(&session_id, &storage_path)
        .await
        .unwrap()
        .unwrap();
    assert!(activated.is_active());
    assert_eq!(activated.objective, "Repair remote login\nand verify");
    assert_ne!(activated.goal_id, created.goal_id);
    coordinator
        .prepare_prompt_thread_goal(&session_id, "/goal Repair remote login\nand verify")
        .await
        .unwrap();
    let retried = coordinator
        .thread_goal_store()
        .get_thread_goal(&session_id, &storage_path)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retried.goal_id, activated.goal_id);
    let invalid = "x".repeat(openbitfun_runtime_ports::MAX_THREAD_GOAL_OBJECTIVE_CHARS + 1);
    assert!(coordinator
        .thread_goal_store()
        .set_thread_goal(
            &session_id,
            &storage_path,
            Some(invalid),
            Some(ThreadGoalStatus::Active),
            None,
            true,
        )
        .await
        .is_err());
    let after_invalid = coordinator
        .thread_goal_store()
        .get_thread_goal(&session_id, &storage_path)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_invalid.goal_id, activated.goal_id);
    assert_eq!(after_invalid.objective, activated.objective);
    assert!(coordinator
        .prepare_prompt_thread_goal(&session_id, "ordinary prompt")
        .await
        .unwrap()
        .is_none());
    assert!(
        session_manager
            .get_session(&session_id)
            .unwrap()
            .dialog_turn_ids
            .is_empty(),
        "goal activation must not submit a duplicate dialog turn"
    );
    if let Some(binding) = session_manager
        .resolve_session_workspace_binding(&session_id)
        .await
    {
        let _ = std::fs::remove_dir_all(binding.session_storage_dir());
    }
}
