use super::super::resolve_agent_session_create_created_by;
use super::super::test_support::{
    register_test_background_task, test_coordinator, test_coordinator_with_config,
    test_coordinator_with_max_active_sessions, test_persistent_coordinator,
};
use crate::agentic::core::{SessionConfig, SessionKind};
use crate::agentic::events::AgenticEvent;
use crate::service::session::SessionStatus;
use openbitfun_runtime_ports::{
    AgentSessionArchiveRequest, AgentSessionCreateRequest, AgentSessionManagementPort,
    AgentSessionRenameRequest, AgentSubmissionPort, AgentThreadGoalGetRequest,
    AgentThreadGoalManagementPort,
};

#[test]
fn agent_session_create_created_by_accepts_camel_case_metadata() {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "createdBy".to_string(),
        serde_json::Value::String("session-parent".to_string()),
    );

    assert_eq!(
        resolve_agent_session_create_created_by(&metadata).as_deref(),
        Some("session-parent")
    );
}

#[test]
fn agent_session_create_created_by_accepts_snake_case_metadata() {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "created_by".to_string(),
        serde_json::Value::String("session-parent".to_string()),
    );

    assert_eq!(
        resolve_agent_session_create_created_by(&metadata).as_deref(),
        Some("session-parent")
    );
}

#[tokio::test]
async fn agent_submission_create_session_preserves_creator_metadata() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-agent-session-port-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&workspace_path).expect("workspace dir should exist");
    let workspace_record =
        crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace_path);
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "createdBy".to_string(),
        serde_json::Value::String("session-parent".to_string()),
    );

    let result = AgentSubmissionPort::create_session(
        &coordinator,
        AgentSessionCreateRequest {
            session_name: "Worker".to_string(),
            agent_type: "Standard".to_string(),
            agent_route_key: None,
            workspace_path: Some(workspace_path.to_string_lossy().into_owned()),
            project_workspace_path: None,
            execution_target: None,
            workspace_id: Some(workspace_record.id.clone()),
            remote_connection_id: None,
            remote_ssh_host: None,
            model_id: Some("explicit-model".to_string()),
            metadata,
        },
    )
    .await
    .expect("port-backed session creation should succeed");
    let created = session_manager
        .get_session(&result.session_id)
        .expect("created session should be persisted");

    assert_eq!(result.session_name, "Worker");
    assert_eq!(result.session_name, created.session_name);
    assert_eq!(created.created_by.as_deref(), Some("session-parent"));
    assert_eq!(
        created.config.workspace_id.as_deref(),
        Some(workspace_record.id.as_str())
    );
    assert_eq!(created.config.model_id.as_deref(), Some("explicit-model"));

    let _ = std::fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn agent_session_management_port_renames_and_sets_persisted_archive_state() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-agent-session-management-port-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&workspace_path).expect("workspace dir should exist");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace_path);
    let workspace = workspace_path.to_string_lossy().into_owned();
    let created = AgentSubmissionPort::create_session(
        &coordinator,
        AgentSessionCreateRequest {
            session_name: "Original".to_string(),
            agent_type: "Standard".to_string(),
            agent_route_key: None,
            workspace_path: Some(workspace.clone()),
            project_workspace_path: None,
            execution_target: None,
            workspace_id: None,
            remote_connection_id: None,
            remote_ssh_host: None,
            model_id: None,
            metadata: serde_json::Map::new(),
        },
    )
    .await
    .expect("session creation should succeed");
    let storage_path = session_manager
        .resolve_session_workspace_binding(&created.session_id)
        .await
        .expect("created session should have a storage binding")
        .session_storage_dir();
    let created_session = session_manager
        .get_session(&created.session_id)
        .expect("created session should stay loaded");
    session_manager
        .persistence_manager()
        .save_session(&storage_path, &created_session)
        .await
        .expect("session fixture should be persisted");

    AgentSessionManagementPort::rename_session(
        &coordinator,
        AgentSessionRenameRequest {
            workspace_id: None,
            workspace_path: workspace.clone(),
            session_id: created.session_id.clone(),
            session_name: "Renamed".to_string(),
            remote_connection_id: None,
            remote_ssh_host: None,
        },
    )
    .await
    .expect("session rename should succeed");
    assert_eq!(
        session_manager
            .get_session(&created.session_id)
            .expect("renamed session should stay loaded")
            .session_name,
        "Renamed"
    );
    let events = coordinator.event_queue.dequeue_batch(10).await;
    assert!(events.iter().any(|envelope| matches!(
        &envelope.event,
        AgenticEvent::SessionTitleGenerated {
            session_id,
            title,
            method,
        } if session_id == &created.session_id && title == "Renamed" && method == "manual"
    )));

    AgentSessionManagementPort::archive_session(
        &coordinator,
        AgentSessionArchiveRequest {
            workspace_id: None,
            workspace_path: workspace.clone(),
            session_id: created.session_id.clone(),
            remote_connection_id: None,
            remote_ssh_host: None,
        },
    )
    .await
    .expect("session archive should succeed");
    let metadata = session_manager
        .persistence_manager()
        .load_session_metadata(&storage_path, &created.session_id)
        .await
        .expect("metadata should load")
        .expect("metadata should exist");
    assert_eq!(metadata.status, SessionStatus::Archived);

    AgentSessionManagementPort::set_session_archived(
        &coordinator,
        openbitfun_runtime_ports::AgentSessionArchiveStateRequest {
            workspace_id: None,
            workspace_path: workspace.clone(),
            session_id: created.session_id.clone(),
            archived: false,
            remote_connection_id: None,
            remote_ssh_host: None,
        },
    )
    .await
    .expect("session unarchive should succeed");
    let metadata = session_manager
        .persistence_manager()
        .load_session_metadata(&storage_path, &created.session_id)
        .await
        .expect("metadata should load")
        .expect("metadata should exist");
    assert_eq!(metadata.status, SessionStatus::Active);

    let _ = std::fs::remove_dir_all(storage_path);
    let _ = std::fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn agent_submission_create_session_preserves_v1_backend_error_classification() {
    let (coordinator, _) = test_coordinator_with_max_active_sessions(0);
    let error = AgentSubmissionPort::create_session(
        &coordinator,
        AgentSessionCreateRequest {
            session_name: "Over capacity".to_string(),
            agent_type: "Standard".to_string(),
            agent_route_key: None,
            workspace_path: Some(std::env::temp_dir().to_string_lossy().into_owned()),
            project_workspace_path: None,
            execution_target: None,
            workspace_id: None,
            remote_connection_id: None,
            remote_ssh_host: None,
            model_id: None,
            metadata: serde_json::Map::new(),
        },
    )
    .await
    .expect_err("v1 create should preserve its backend error classification");

    assert_eq!(error.kind, openbitfun_runtime_ports::PortErrorKind::Backend);
}

#[tokio::test]
async fn agent_submission_create_session_preserves_requested_session_id() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-agent-session-fixed-id-port-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&workspace_path).expect("workspace dir should exist");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace_path);

    let result = AgentSubmissionPort::create_session_with_id(
        &coordinator,
        "fixed-session-id".to_string(),
        AgentSessionCreateRequest {
            session_name: "Fixed worker".to_string(),
            agent_type: "Standard".to_string(),
            agent_route_key: None,
            workspace_path: Some(workspace_path.to_string_lossy().into_owned()),
            project_workspace_path: None,
            execution_target: None,
            workspace_id: None,
            remote_connection_id: None,
            remote_ssh_host: None,
            model_id: None,
            metadata: serde_json::Map::new(),
        },
    )
    .await
    .expect("fixed-id session creation should succeed");

    assert_eq!(result.session_id, "fixed-session-id");
    assert!(session_manager.get_session("fixed-session-id").is_some());
    let default_workspace_goal = AgentThreadGoalManagementPort::get_thread_goal(
        &coordinator,
        AgentThreadGoalGetRequest {
            session_id: "fixed-session-id".to_string(),
            workspace_path: ".".to_string(),
            remote_connection_id: None,
            remote_ssh_host: None,
        },
    )
    .await
    .expect("loaded local session should accept the default workspace");
    assert_eq!(default_workspace_goal, None);

    let duplicate_error = AgentSubmissionPort::create_session_with_id(
        &coordinator,
        "fixed-session-id".to_string(),
        AgentSessionCreateRequest {
            session_name: "Duplicate worker".to_string(),
            agent_type: "Standard".to_string(),
            agent_route_key: None,
            workspace_path: Some(workspace_path.to_string_lossy().into_owned()),
            project_workspace_path: None,
            execution_target: None,
            workspace_id: None,
            remote_connection_id: None,
            remote_ssh_host: None,
            model_id: None,
            metadata: serde_json::Map::new(),
        },
    )
    .await
    .expect_err("duplicate fixed session id should be rejected");
    assert_eq!(
        duplicate_error.kind,
        openbitfun_runtime_ports::PortErrorKind::InvalidRequest
    );
    assert!(duplicate_error.message.starts_with("Validation error:"));
    assert!(duplicate_error.message.contains("already exists"));
    assert_eq!(
        session_manager
            .get_session("fixed-session-id")
            .expect("original fixed-id session should remain")
            .session_name,
        "Fixed worker"
    );
    assert!(session_manager
        .unload_session_from_memory("fixed-session-id")
        .await
        .expect("fixed-id session should unload"));
    // Once the session is unloaded there is no in-memory workspace to
    // inherit and "." is not a registered workspace record, so the read
    // must fail loudly instead of guessing a current-directory store.
    let unloaded_default_workspace_error = AgentThreadGoalManagementPort::get_thread_goal(
        &coordinator,
        AgentThreadGoalGetRequest {
            session_id: "fixed-session-id".to_string(),
            workspace_path: ".".to_string(),
            remote_connection_id: None,
            remote_ssh_host: None,
        },
    )
    .await
    .expect_err("unloaded session without a workspace scope must not resolve storage");
    assert!(
        unloaded_default_workspace_error
            .message
            .contains("does not resolve to a local workspace"),
        "{}",
        unloaded_default_workspace_error.message
    );
}

#[tokio::test]
async fn runtime_default_title_is_shared_and_explicit_remote_titles_stay_literal() {
    let (coordinator, session_manager) = test_persistent_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-title-contract-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&workspace_path).unwrap();
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace_path);
    let expected = crate::service::i18n::generated_locale_contract::generated_shared_term(
        crate::service::config::app_language::get_app_language().await,
        "sessions.new",
    )
    .unwrap();
    for (agent_type, title, descriptor) in [
        ("Standard", "", false),
        ("Cowork", "  ", false),
        ("Claw", "", false),
        ("Claw", "Remote Claw Session", false),
        ("Standard", expected, false),
        ("Standard", "New Session", true),
    ] {
        let created = AgentSubmissionPort::create_session(
                &coordinator,
                AgentSessionCreateRequest {
                    session_name: title.into(),
                    agent_type: agent_type.into(),
                    agent_route_key: None,
                    workspace_path: Some(workspace_path.to_string_lossy().into_owned()),
                    project_workspace_path: None,
                    execution_target: None,
                    workspace_id: None,
                    remote_connection_id: None,
                    remote_ssh_host: None,
                    model_id: None,
                    metadata: serde_json::from_value(
                        if descriptor {
                            serde_json::json!({ "titleSource": "i18n", "titleKey": "flow-chat:session.new", "titleParams": { "defaultTitleText": title } })
                        } else { serde_json::json!({ "source": "remote_relay" }) },
                    )
                    .unwrap(),
                },
            )
            .await
            .unwrap();
        let path = session_manager
            .resolve_session_workspace_binding(&created.session_id)
            .await
            .unwrap()
            .session_storage_dir();
        let metadata = session_manager
            .persistence_manager()
            .load_session_metadata(&path, &created.session_id)
            .await
            .unwrap()
            .unwrap();
        let is_default = descriptor || title.trim().is_empty();
        assert_eq!(
            created.session_name,
            if is_default { expected } else { title }
        );
        assert_eq!(
            openbitfun_services_core::session::naming::is_default_title(&metadata),
            is_default
        );
        assert_eq!(
            session_manager
                .update_session_title_if_current(
                    &created.session_id,
                    &created.session_name,
                    "Generated topic"
                )
                .await
                .unwrap(),
            is_default
        );
    }
    let _ = std::fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn transient_session_port_never_persists_or_discards_a_durable_identity() {
    let (coordinator, session_manager) = test_coordinator_with_config(100, true);
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-agent-transient-session-port-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&workspace_path).expect("workspace dir should exist");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace_path);
    let workspace = workspace_path.to_string_lossy().into_owned();
    let request = |name: &str| AgentSessionCreateRequest {
        session_name: name.to_string(),
        agent_type: "Standard".to_string(),
        agent_route_key: None,
        workspace_path: Some(workspace.clone()),
        project_workspace_path: None,
        execution_target: None,
        workspace_id: None,
        remote_connection_id: None,
        remote_ssh_host: None,
        model_id: None,
        metadata: serde_json::Map::new(),
    };

    let transient = AgentSubmissionPort::create_transient_session_with_id(
        &coordinator,
        "transient-session".to_string(),
        request("Transient"),
    )
    .await
    .expect("transient Session creation should succeed");
    let loaded = session_manager
        .get_session(&transient.session_id)
        .expect("transient Session should be loaded");
    assert!(session_manager.is_transient_session(&loaded.session_id));
    let storage_path = session_manager
        .resolve_session_workspace_binding(&transient.session_id)
        .await
        .expect("transient Session should retain its workspace binding")
        .session_storage_dir();
    assert!(!session_manager
        .persistence_manager()
        .session_storage_exists(&workspace_path, &transient.session_id)
        .expect("persistence probe should succeed"));

    let transient_child = session_manager
        .create_transient_session_with_id_and_details(
            None,
            "Transient child".to_string(),
            "Explore".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.clone()),
                ..Default::default()
            },
            Some(format!("session-{}", transient.session_id)),
            SessionKind::Subagent,
        )
        .await
        .expect("transient child Session should be created");
    let transient_grandchild = session_manager
        .create_transient_session_with_id_and_details(
            None,
            "Transient grandchild".to_string(),
            "Explore".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.clone()),
                ..Default::default()
            },
            Some(format!("session-{}", transient_child.session_id)),
            SessionKind::Subagent,
        )
        .await
        .expect("nested transient child Session should be created");
    let background_task = register_test_background_task(
        &coordinator,
        &transient.session_id,
        "transient-parent-turn",
        &transient_child.session_id,
    )
    .await;

    let discarded = coordinator
        .discard_transient_session(&workspace_path, None, None, &transient.session_id)
        .await
        .expect("owned transient Session discard should succeed");
    assert!(discarded);
    assert!(session_manager.get_session(&transient.session_id).is_none());
    assert!(
        session_manager
            .get_session(&transient_child.session_id)
            .is_none(),
        "discarding a transient parent must release its transient descendants"
    );
    assert!(session_manager
        .get_session(&transient_grandchild.session_id)
        .is_none());
    assert!(session_manager
        .resolve_session_workspace_binding(&transient.session_id)
        .await
        .is_none());
    assert!(coordinator
        .background_subagent_outcomes
        .resolve_agent_id(&transient.session_id, &background_task.agent_id)
        .await
        .is_err());

    let durable = AgentSubmissionPort::create_session_with_id(
        &coordinator,
        "durable-session".to_string(),
        request("Durable"),
    )
    .await
    .expect("durable Session creation should succeed");
    let discard_error = session_manager
        .discard_transient_session(&workspace_path, None, None, &durable.session_id)
        .await
        .expect_err("transient discard must reject durable Session ownership");
    assert!(matches!(
        discard_error,
        crate::util::errors::OpenBitFunError::Validation(_)
    ));
    session_manager.evict_loaded_session_for_test(&durable.session_id);
    let collision = AgentSubmissionPort::create_transient_session_with_id(
        &coordinator,
        durable.session_id.clone(),
        request("Collision"),
    )
    .await
    .expect_err("transient Session must not shadow persisted durable identity");
    assert_eq!(
        collision.kind,
        openbitfun_runtime_ports::PortErrorKind::InvalidRequest
    );

    let _ = std::fs::remove_dir_all(storage_path);
    let _ = std::fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn agent_submission_create_session_rejects_invalid_requested_session_id() {
    let (coordinator, _) = test_coordinator();
    let error = AgentSubmissionPort::create_session_with_id(
        &coordinator,
        "../other-session".to_string(),
        AgentSessionCreateRequest {
            session_name: "Invalid worker".to_string(),
            agent_type: "Standard".to_string(),
            agent_route_key: None,
            workspace_path: Some(std::env::temp_dir().to_string_lossy().into_owned()),
            project_workspace_path: None,
            execution_target: None,
            workspace_id: None,
            remote_connection_id: None,
            remote_ssh_host: None,
            model_id: None,
            metadata: serde_json::Map::new(),
        },
    )
    .await
    .expect_err("invalid fixed session id should be rejected");

    assert_eq!(
        error.kind,
        openbitfun_runtime_ports::PortErrorKind::InvalidRequest
    );
    assert!(error.message.starts_with("Validation error:"));
}
