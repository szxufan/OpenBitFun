use super::super::{
    SessionExecutionBindingUpdate, TurnAdmissionSessionFacts, TEST_MODEL_RESOLUTION_AI_CONFIG,
};
use super::support::{test_ai_config, test_manager, test_model, TestWorkspace};
use crate::agentic::core::{
    Message, MessageContent, MessageRole, Session, SessionAgentRouteOwner, SessionConfig,
    SessionModelBindingPolicy, SessionState,
};
use crate::agentic::persistence::PersistenceManager;
use crate::service::session::TurnStatus;
use openbitfun_core_types::SessionExecutionTarget;
use std::sync::Arc;

#[tokio::test]
async fn dialog_turn_admission_rejects_a_concurrent_session_model_change() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Turn admission CAS".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                model_id: Some("model-original".to_string()),
                reasoning_preset: Some("high".to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let expected = TurnAdmissionSessionFacts::from_session(&session);
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("session should remain loaded")
        .config
        .model_id = Some("model-updated".to_string());

    let error = manager
        .start_dialog_turn_with_prepended_messages_if_session_matches(
            &session.session_id,
            "Standard".to_string(),
            "must retry admission".to_string(),
            Some("turn-admission-race".to_string()),
            None,
            Vec::new(),
            Some(serde_json::json!({"runtime_resolved_model_id": "model-original"})),
            &expected,
        )
        .await
        .expect_err("a concurrent settings update must invalidate admission");

    assert!(error.to_string().contains("changed during turn admission"));
    assert_eq!(manager.get_turn_count(&session.session_id), 0);
}

#[tokio::test]
async fn dialog_turn_admission_rejects_a_concurrent_context_window_change() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Turn admission context window CAS".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                model_id: Some("model-original".to_string()),
                max_context_tokens: 128_128,
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let expected = TurnAdmissionSessionFacts::from_session(&session);
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            test_ai_config(vec![test_model("model-original", 256_000)]),
            manager.update_session_model_selection(&session.session_id, "model-original", None),
        )
        .await
        .expect("same-model context window refresh should succeed");

    let error = manager
        .start_dialog_turn_with_prepended_messages_if_session_matches(
            &session.session_id,
            "Standard".to_string(),
            "must reject stale context window".to_string(),
            Some("turn-admission-context-window-race".to_string()),
            None,
            Vec::new(),
            None,
            &expected,
        )
        .await
        .expect_err("a concurrent context window update must invalidate admission");

    assert!(error.to_string().contains("changed during turn admission"));
    assert_eq!(manager.get_turn_count(&session.session_id), 0);
}

#[tokio::test]
async fn dialog_turn_admission_rejects_a_concurrent_agent_route_owner_change() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Turn admission route CAS".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let expected = TurnAdmissionSessionFacts::from_session(&session);
    manager
        .update_session_agent_binding(
            &session.session_id,
            "Standard",
            SessionAgentRouteOwner::External,
            None,
        )
        .await
        .expect("same-name route owner update should succeed");

    let error = manager
        .start_dialog_turn_with_prepended_messages_if_session_matches(
            &session.session_id,
            "Standard".to_string(),
            "must reject stale route owner".to_string(),
            Some("turn-admission-route-race".to_string()),
            None,
            Vec::new(),
            None,
            &expected,
        )
        .await
        .expect_err("a concurrent route owner update must invalidate admission");

    assert!(error.to_string().contains("changed during turn admission"));
    assert_eq!(manager.get_turn_count(&session.session_id), 0);
}

#[tokio::test]
async fn dialog_turn_admission_rejects_a_concurrent_agent_route_key_change() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Turn admission route key CAS".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                agent_route_key: Some("local:agentic:v1".to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let expected = TurnAdmissionSessionFacts::from_session(&session);
    manager
        .update_session_agent_binding(
            &session.session_id,
            "Standard",
            SessionAgentRouteOwner::Local,
            Some("local:agentic:v2".to_string()),
        )
        .await
        .expect("same-owner route key update should succeed");

    let error = manager
        .start_dialog_turn_with_prepended_messages_if_session_matches(
            &session.session_id,
            "Standard".to_string(),
            "must reject stale route key".to_string(),
            Some("turn-admission-route-key-race".to_string()),
            None,
            Vec::new(),
            None,
            &expected,
        )
        .await
        .expect_err("a concurrent route key update must invalidate admission");

    assert!(error.to_string().contains("changed during turn admission"));
    assert_eq!(manager.get_turn_count(&session.session_id), 0);
}

#[tokio::test]
async fn dialog_turn_admission_rejects_a_concurrent_execution_binding_change() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let original_workspace = workspace.path().to_string_lossy().to_string();
    let session = manager
        .create_session(
            "Turn admission workspace CAS".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(original_workspace.clone()),
                project_workspace_path: Some(original_workspace.clone()),
                execution_target: Some(SessionExecutionTarget::local(original_workspace.clone())),
                workspace_id: Some(workspace.workspace_id().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let expected = TurnAdmissionSessionFacts::from_session(&session);
    let rebound_workspace = workspace.path().join("managed-worktree");
    manager
        .update_session_execution_binding(
            &session.session_id,
            SessionExecutionBindingUpdate {
                workspace_path: rebound_workspace.to_string_lossy().to_string(),
                project_workspace_path: original_workspace,
                workspace_id: Some("workspace-rebound".to_string()),
                execution_target: SessionExecutionTarget::local(
                    rebound_workspace.to_string_lossy().to_string(),
                ),
            },
        )
        .await
        .expect("execution binding update should succeed before the first turn");

    let error = manager
        .start_dialog_turn_with_prepended_messages_if_session_matches(
            &session.session_id,
            "Standard".to_string(),
            "must reject stale workspace binding".to_string(),
            Some("turn-admission-workspace-race".to_string()),
            None,
            Vec::new(),
            None,
            &expected,
        )
        .await
        .expect_err("a concurrent execution binding update must invalidate admission");

    assert!(error.to_string().contains("changed during turn admission"));
    assert_eq!(manager.get_turn_count(&session.session_id), 0);
}

#[tokio::test]
async fn start_dialog_turn_with_existing_context_persists_turn_and_snapshot() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Fork child".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    let seeded_messages = vec![
        Message::user("fork reminder".to_string()),
        Message::assistant("inherited context".to_string()),
    ];
    manager
        .replace_context_messages(&session.session_id, seeded_messages.clone())
        .await;

    let turn_id = manager
        .start_dialog_turn_with_existing_context(
            &session.session_id,
            "Standard".to_string(),
            "delegate task".to_string(),
            Some("subagent-turn-0".to_string()),
            None,
        )
        .await
        .expect("turn should start");

    assert_eq!(turn_id, "subagent-turn-0");
    assert_eq!(
        manager
            .get_session(&session.session_id)
            .expect("session should remain in memory")
            .dialog_turn_ids,
        vec!["subagent-turn-0".to_string()]
    );

    let persisted_turn = persistence_manager
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .expect("turn load should succeed")
        .expect("turn should exist");
    assert_eq!(persisted_turn.turn_id, "subagent-turn-0");
    assert_eq!(persisted_turn.user_message.content, "delegate task");

    let snapshot = persistence_manager
        .load_turn_context_snapshot(workspace.path(), &session.session_id, 0)
        .await
        .expect("snapshot load should succeed")
        .expect("snapshot should exist");
    assert_eq!(snapshot.len(), seeded_messages.len());
    assert!(matches!(snapshot[0].role, MessageRole::User));
    assert!(matches!(snapshot[1].role, MessageRole::Assistant));
    assert!(matches!(
        &snapshot[0].content,
        MessageContent::Text(text) if text == "fork reminder"
    ));
    assert!(matches!(
        &snapshot[1].content,
        MessageContent::Text(text) if text == "inherited context"
    ));

    let runtime_context = manager
        .get_context_messages(&session.session_id)
        .await
        .expect("runtime context should remain readable");
    assert_eq!(runtime_context.len(), seeded_messages.len());
}

#[tokio::test]
async fn dialog_and_maintenance_turn_admission_is_atomic() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Atomic admission".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    let dialog = manager.start_dialog_turn(
        &session.session_id,
        "Standard".to_string(),
        "new user input".to_string(),
        Some("dialog-turn".to_string()),
        None,
        None,
    );
    let maintenance = manager.start_maintenance_turn(
        &session.session_id,
        "/compact".to_string(),
        Some("compact-turn".to_string()),
        None,
    );
    let (dialog_result, maintenance_result) = tokio::join!(dialog, maintenance);

    assert_eq!(
        usize::from(dialog_result.is_ok()) + usize::from(maintenance_result.is_ok()),
        1,
        "only one competing turn may acquire an idle session"
    );
    let active = manager
        .get_session(&session.session_id)
        .expect("session should remain available");
    assert_eq!(active.dialog_turn_ids.len(), 1);
    let accepted_turn = dialog_result
        .ok()
        .or_else(|| maintenance_result.ok())
        .expect("one turn should be admitted");
    assert!(matches!(
        active.state,
        SessionState::Processing {
            ref current_turn_id,
            ..
        } if current_turn_id == &accepted_turn
    ));
}

#[tokio::test]
async fn user_dialog_retry_is_allowed_from_error_but_maintenance_is_not() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Retry admission".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    {
        let mut active = manager
            .sessions
            .get_mut(&session.session_id)
            .expect("session should remain available");
        active.state = SessionState::Error {
            error: "retryable failure".to_string(),
            recoverable: true,
        };
    }

    let maintenance = manager
        .start_maintenance_turn(
            &session.session_id,
            "/compact".to_string(),
            Some("compact-turn".to_string()),
            None,
        )
        .await;
    assert!(
        maintenance.is_err(),
        "maintenance work must remain idle-only"
    );

    let retry_turn = manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "retry after failure".to_string(),
            Some("retry-turn".to_string()),
            None,
            None,
        )
        .await
        .expect("ordinary dialog should preserve error-state retry semantics");
    assert_eq!(retry_turn, "retry-turn");
}

#[tokio::test]
async fn maintenance_failure_persists_its_terminal_error() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Failed maintenance".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = manager
        .start_maintenance_turn(
            &session.session_id,
            "/compact".to_string(),
            Some("compact-turn".to_string()),
            None,
        )
        .await
        .expect("maintenance turn should start");

    manager
        .fail_maintenance_turn(
            &session.session_id,
            &turn_id,
            "terminal persistence failed".to_string(),
            Vec::new(),
        )
        .await
        .expect("maintenance failure should persist");

    let _mutation = manager
        .acquire_session_mutation(&session.session_id)
        .await
        .expect("transcript mutation");
    let turns = manager
        .load_persisted_transcript_turns_locked(&session.session_id)
        .await
        .expect("turns should load")
        .expect("persistence should be enabled");
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].status, TurnStatus::Error);
    assert_eq!(turns[0].finish_reason.as_deref(), Some("failed"));
    assert_eq!(turns[0].has_final_response, Some(false));
    assert_eq!(
        turns[0].error.as_deref(),
        Some("terminal persistence failed")
    );
}

#[test]
fn model_pool_admission_fences_pool_and_selection_revision() {
    let mut session = Session::new("Pool".into(), "Standard".into(), SessionConfig::default());
    session.config.model_binding_policy = SessionModelBindingPolicy::Pool;
    session.config.model_pool_id = Some("p".into());
    let facts = super::super::TurnAdmissionSessionFacts::from_session(&session);
    assert!(facts.matches(&session));
    session.config.model_pool_id = Some("q".into());
    assert!(!facts.matches(&session));
    session.config.model_pool_id = Some("p".into());
    session.config.model_pool_selection_revision = 1;
    assert!(!facts.matches(&session));
}
