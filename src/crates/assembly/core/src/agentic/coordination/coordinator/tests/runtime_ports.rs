use super::super::test_support::{test_coordinator, test_persistent_coordinator};
use super::super::{
    runtime_port_error_preserving_message, ConversationCoordinator, TEST_AGENT_MODEL_DEFAULTS,
};
use crate::agentic::core::SessionConfig;
use openbitfun_config_contracts::model_pool::AgentModelSelections;
use std::sync::Arc;

#[test]
fn migrated_runtime_ports_preserve_existing_core_error_messages() {
    let error = runtime_port_error_preserving_message(
        crate::util::errors::OpenBitFunError::Validation("invalid session id".to_string()),
    );

    assert_eq!(
        error.kind,
        openbitfun_runtime_ports::PortErrorKind::InvalidRequest
    );
    assert_eq!(error.message, "Validation error: invalid session id");
}

#[tokio::test]
async fn interaction_response_port_uses_user_question_owner_and_typed_stale_errors() {
    use openbitfun_agent_runtime::sdk::{AgentInteractionResponsePort, AgentUserAnswersRequest};

    let (coordinator, _) = test_coordinator();
    let answer_tool_id = format!("answer-{}", uuid::Uuid::new_v4());
    let (sender, receiver) = tokio::sync::oneshot::channel::<
        openbitfun_agent_runtime::user_questions::UserInputResponse,
    >();
    crate::agentic::tools::user_input_manager::get_user_input_manager()
        .register_channel(answer_tool_id.clone(), sender);

    AgentInteractionResponsePort::submit_user_answers(
        &coordinator,
        AgentUserAnswersRequest {
            tool_id: answer_tool_id.clone(),
            answers: serde_json::json!({ "0": "continue" }),
        },
    )
    .await
    .expect("deliver user answers through the Core-owned channel");
    assert_eq!(
        receiver.await.expect("receive user answers").answers,
        serde_json::json!({ "0": "continue" })
    );

    let stale_answer = AgentInteractionResponsePort::submit_user_answers(
        &coordinator,
        AgentUserAnswersRequest {
            tool_id: answer_tool_id.clone(),
            answers: serde_json::json!({ "0": "continue" }),
        },
    )
    .await
    .expect_err("consumed answer channel must be reported as stale");
    assert_eq!(
        stale_answer.kind,
        openbitfun_runtime_ports::PortErrorKind::NotFound
    );
    assert_eq!(
        stale_answer.message,
        format!("Tool error: Waiting channel not found: {answer_tool_id}")
    );
}

#[tokio::test]
async fn session_model_port_preserves_core_not_found_errors() {
    use openbitfun_agent_runtime::sdk::{AgentSessionModelPort, AgentSessionModelUpdateRequest};

    let (coordinator, _) = test_coordinator();
    let error = AgentSessionModelPort::update_session_model(
        &coordinator,
        AgentSessionModelUpdateRequest {
            session_id: "missing-session".to_string(),
            model_id: "primary".to_string(),
        },
    )
    .await
    .expect_err("missing session must remain a typed not-found error");

    assert_eq!(
        error.kind,
        openbitfun_runtime_ports::PortErrorKind::NotFound
    );
    assert!(error.message.contains("missing-session"));
}

#[tokio::test]
async fn session_mode_port_preserves_core_not_found_errors() {
    use openbitfun_agent_runtime::sdk::{AgentSessionModePort, AgentSessionModeUpdateRequest};

    let (coordinator, _) = test_coordinator();
    let error = AgentSessionModePort::update_session_mode(
        &coordinator,
        AgentSessionModeUpdateRequest {
            session_id: "missing-session".to_string(),
            mode_id: "Standard".to_string(),
            agent_route_key: None,
        },
    )
    .await
    .expect_err("missing session must remain a typed not-found error");

    assert_eq!(
        error.kind,
        openbitfun_runtime_ports::PortErrorKind::NotFound
    );
    assert!(error.message.contains("missing-session"));
}

#[tokio::test]
async fn session_mode_port_rejects_blank_mode_for_active_session() {
    use openbitfun_agent_runtime::sdk::{AgentSessionModePort, AgentSessionModeUpdateRequest};

    let (coordinator, _) = test_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-session-mode-validation-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&workspace_path).expect("workspace dir should exist");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace_path);
    let workspace_path_string = workspace_path.to_string_lossy().into_owned();
    let session = TEST_AGENT_MODEL_DEFAULTS
        .scope(
            AgentModelSelections::default(),
            coordinator.create_session_with_workspace(
                None,
                "Runtime mode validation".to_string(),
                "Standard".to_string(),
                SessionConfig {
                    tool_presentation_mode: Default::default(),
                    workspace_path: Some(workspace_path_string.clone()),
                    ..Default::default()
                },
                workspace_path_string,
            ),
        )
        .await
        .expect("real Core session should be created");

    let error = AgentSessionModePort::update_session_mode(
        &coordinator,
        AgentSessionModeUpdateRequest {
            session_id: session.session_id,
            mode_id: "   ".to_string(),
            agent_route_key: None,
        },
    )
    .await
    .expect_err("blank mode must remain a typed invalid request");

    assert_eq!(
        error.kind,
        openbitfun_runtime_ports::PortErrorKind::InvalidRequest
    );
    let _ = std::fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn session_mode_port_rejects_unknown_mode_for_active_session() {
    use openbitfun_agent_runtime::sdk::{AgentSessionModePort, AgentSessionModeUpdateRequest};

    let (coordinator, _) = test_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-session-mode-validation-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&workspace_path).expect("workspace dir should exist");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace_path);
    let workspace_path_string = workspace_path.to_string_lossy().into_owned();
    let session = TEST_AGENT_MODEL_DEFAULTS
        .scope(
            AgentModelSelections::default(),
            coordinator.create_session_with_workspace(
                None,
                "Runtime mode validation".to_string(),
                "Standard".to_string(),
                SessionConfig {
                    tool_presentation_mode: Default::default(),
                    workspace_path: Some(workspace_path_string.clone()),
                    ..Default::default()
                },
                workspace_path_string,
            ),
        )
        .await
        .expect("real Core session should be created");

    let error = AgentSessionModePort::update_session_mode(
        &coordinator,
        AgentSessionModeUpdateRequest {
            session_id: session.session_id,
            mode_id: "__missing_runtime_mode__".to_string(),
            agent_route_key: None,
        },
    )
    .await
    .expect_err("unknown mode must remain a typed invalid request");

    assert_eq!(
        error.kind,
        openbitfun_runtime_ports::PortErrorKind::InvalidRequest
    );
    let _ = std::fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn session_mode_runtime_updates_the_real_core_session() {
    use openbitfun_agent_runtime::sdk::{AgentRuntimeBuilder, AgentSessionModeUpdateRequest};

    let (coordinator, session_manager) = test_coordinator();
    let coordinator = Arc::new(coordinator);
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-session-mode-runtime-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&workspace_path).expect("workspace dir should exist");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace_path);
    let workspace_path_string = workspace_path.to_string_lossy().into_owned();
    let session = TEST_AGENT_MODEL_DEFAULTS
        .scope(
            AgentModelSelections::default(),
            coordinator.create_session_with_workspace(
                None,
                "Runtime mode update".to_string(),
                "Standard".to_string(),
                SessionConfig {
                    tool_presentation_mode: Default::default(),
                    workspace_path: Some(workspace_path_string.clone()),
                    ..Default::default()
                },
                workspace_path_string,
            ),
        )
        .await
        .expect("real Core session should be created");
    let runtime = AgentRuntimeBuilder::new()
        .with_submission_port(coordinator.clone())
        .with_session_mode_port(coordinator)
        .build()
        .expect("assembled agent runtime");

    runtime
        .update_session_mode(AgentSessionModeUpdateRequest {
            session_id: session.session_id.clone(),
            mode_id: " Cowork ".to_string(),
            agent_route_key: None,
        })
        .await
        .expect("runtime mode port should update the Core owner");

    assert_eq!(
        session_manager
            .get_session(&session.session_id)
            .map(|session| session.agent_type.clone())
            .as_deref(),
        Some("Cowork")
    );
    let _ = std::fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn session_model_runtime_updates_the_real_core_session() {
    use openbitfun_agent_runtime::sdk::{AgentRuntimeBuilder, AgentSessionModelUpdateRequest};

    let (coordinator, session_manager) = test_coordinator();
    let coordinator = Arc::new(coordinator);
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-session-model-runtime-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&workspace_path).expect("workspace dir should exist");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace_path);
    let workspace_path_string = workspace_path.to_string_lossy().into_owned();
    let session = TEST_AGENT_MODEL_DEFAULTS
        .scope(
            AgentModelSelections::default(),
            coordinator.create_session_with_workspace(
                None,
                "Runtime model update".to_string(),
                "Standard".to_string(),
                SessionConfig {
                    tool_presentation_mode: Default::default(),
                    workspace_path: Some(workspace_path_string.clone()),
                    model_id: Some("primary".to_string()),
                    ..Default::default()
                },
                workspace_path_string,
            ),
        )
        .await
        .expect("real Core session should be created");
    let runtime = AgentRuntimeBuilder::new()
        .with_submission_port(coordinator.clone())
        .with_session_model_port(coordinator)
        .build()
        .expect("assembled agent runtime");

    runtime
        .update_session_model(AgentSessionModelUpdateRequest {
            session_id: session.session_id.clone(),
            model_id: " default ".to_string(),
        })
        .await
        .expect("runtime model port should update the Core owner");

    assert_eq!(
        session_manager
            .get_session(&session.session_id)
            .and_then(|session| session.config.model_id.clone())
            .as_deref(),
        Some("primary")
    );
    let _ = std::fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn control_conversation_reset_preserves_history_and_retries_one_selection() {
    let workspace = tempfile::tempdir().expect("control workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    // Installs the global workspace service the control conversation
    // registers its app-owned folder with.
    crate::service::workspace::legacy_compat::register_local_fixture(workspace.path(), None).await;
    let (coordinator, manager) = test_persistent_coordinator();
    let original = coordinator
        .select_control_conversation_in_workspace(workspace.path(), None)
        .await
        .unwrap();
    assert_eq!(original.session_id, "openbitfun-control");
    assert!(!original.workspace_id.is_empty());
    assert_eq!(
        manager
            .get_session(&original.session_id)
            .unwrap()
            .config
            .workspace_id
            .as_deref(),
        Some(original.workspace_id.as_str())
    );
    coordinator
        .record_voice_exchange(super::super::super::VoiceExchangeRequest {
            session_id: original.session_id.clone(),
            exchange_id: "saved-exchange".into(),
            user_text: "Keep this record".into(),
            assistant_text: "Saved".into(),
        })
        .await
        .unwrap();
    let before = manager
        .persistence_manager()
        .load_session_metadata(workspace.path(), &original.session_id)
        .await
        .unwrap()
        .unwrap();

    let (first, duplicate) = tokio::join!(
        coordinator
            .select_control_conversation_in_workspace(workspace.path(), Some(&original.session_id)),
        coordinator
            .select_control_conversation_in_workspace(workspace.path(), Some(&original.session_id)),
    );
    let created = first.unwrap();
    assert_ne!(created.session_id, original.session_id);
    assert_eq!(duplicate.unwrap().session_id, created.session_id);
    assert_eq!(
        coordinator
            .select_control_conversation_in_workspace(workspace.path(), None)
            .await
            .unwrap()
            .session_id,
        created.session_id
    );
    let after = manager
        .persistence_manager()
        .load_session_metadata(workspace.path(), &original.session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
    assert!(manager.get_session(&original.session_id).is_some());

    let state_path = workspace.path().join("active.json");
    tokio::fs::write(&state_path, "unreadable saved selection")
        .await
        .unwrap();
    assert!(coordinator
        .select_control_conversation_in_workspace(workspace.path(), None)
        .await
        .is_err());
    assert_eq!(
        tokio::fs::read_to_string(state_path).await.unwrap(),
        "unreadable saved selection"
    );
}

#[test]
fn conversation_coordinator_exposes_remote_runtime_ports() {
    fn assert_cancellation_port<T: openbitfun_runtime_ports::AgentTurnCancellationPort>() {}
    fn assert_state_port<T: openbitfun_runtime_ports::RemoteControlStatePort>() {}

    assert_cancellation_port::<ConversationCoordinator>();
    assert_state_port::<ConversationCoordinator>();
}

#[tokio::test]
async fn coordinator_test_fixture_injects_terminal_port() {
    let (coordinator, _) = test_coordinator();

    assert!(coordinator.terminal_port().is_some());
    assert!(coordinator.remote_exec_port().is_some());
}
