use super::super::{PermissionMode, TEST_MODEL_RESOLUTION_AI_CONFIG};
use super::support::{
    configured_reasoning_model, interrupted_turn_test_model_fingerprint,
    interrupted_turn_test_reasoning_fingerprint, reopen_interrupted_turn_for_test, test_ai_config,
    test_manager, TestWorkspace,
};
use crate::agentic::core::{
    Message, MessageContent, MessageSemanticKind, SessionConfig, SessionState, ToolCall, TurnStats,
};
use crate::agentic::persistence::PersistenceManager;
use crate::infrastructure::ai::reasoning_catalog::{
    project_model_reasoning_catalog as project_test_model_reasoning_catalog,
    reasoning_preset_runtime_fingerprint as test_reasoning_preset_runtime_fingerprint,
    resolve_default_reasoning_preset as resolve_test_default_reasoning_preset,
};
use crate::service::config::types::{
    model_runtime_binding_fingerprint as service_model_runtime_binding_fingerprint,
    AIModelConfig as ServiceAIModelConfig,
};
use crate::service::session::{DialogTurnRecoveryStatus, TurnStatus};
use openbitfun_core_types::{
    ReasoningCatalogBinding, ReasoningConfig, ReasoningPreset, ReasoningPresetAction,
};
use serde_json::json;
use std::sync::Arc;

fn interrupted_turn_test_auto_reasoning_fingerprint(default_preset: &str) -> String {
    let model = reasoning_model_with_default("model-original", default_preset);
    let projection = project_test_model_reasoning_catalog(&model, None);
    test_reasoning_preset_runtime_fingerprint(resolve_test_default_reasoning_preset(&projection))
}

#[tokio::test]
async fn interrupted_turn_reopens_same_turn_from_snapshot_with_generation_cas() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Recovery".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "finish the task".to_string(),
            Some("turn-1".to_string()),
            None,
            Some(serde_json::json!({
                "resolved_permission_mode": "ask",
                "runtime_resolved_model_id": "model-original",
                "runtime_model_binding_fingerprint": interrupted_turn_test_model_fingerprint(),
                "runtime_reasoning_preset": null,
                "runtime_reasoning_selection": null,
                "runtime_reasoning_fingerprint": interrupted_turn_test_reasoning_fingerprint(None)
            })),
        )
        .await
        .expect("turn should start");
    manager
        .add_message(
            &session.session_id,
            Message::assistant("safe completed fragment".to_string())
                .with_turn_id(turn_id.clone())
                .with_round_id("round-0".to_string()),
        )
        .await
        .expect("safe boundary should persist");
    manager
        .add_message(
            &session.session_id,
            Message::assistant_with_tools(
                String::new(),
                vec![ToolCall {
                    tool_id: "in-flight-recovery-call".to_string(),
                    tool_name: "Read".to_string(),
                    arguments: json!({"path": "still-running.txt"}),
                    ..ToolCall::default()
                }],
            )
            .with_turn_id(turn_id.clone())
            .with_round_id("round-1".to_string()),
        )
        .await
        .expect("in-flight tool call should persist");

    let interrupted = manager
        .mark_dialog_turn_interrupted(&session.session_id, &turn_id)
        .await
        .expect("turn should become interrupted");
    assert_eq!(interrupted.execution_generation, 0);
    manager
        .update_session_state_for_turn_if_processing(
            &session.session_id,
            &turn_id,
            SessionState::Idle,
        )
        .await
        .expect("session should settle idle");
    manager
        .update_session_permission_mode(&session.session_id, Some(PermissionMode::FullAccess))
        .await
        .expect("session permission should change after interruption");

    let plan = reopen_interrupted_turn_for_test(&manager, &session.session_id, &turn_id, 0)
        .await
        .expect("same generation should recover");

    assert_eq!(plan.turn_id, turn_id);
    assert_eq!(plan.turn_index, 0);
    assert_eq!(plan.execution_generation, 1);
    assert_eq!(plan.resume_count, 1);
    assert_eq!(plan.initial_round_index, 0);
    assert_eq!(plan.resolved_permission_mode, PermissionMode::Ask);
    assert_eq!(plan.resolved_model_id, "model-original");
    assert_eq!(
        plan.messages
            .iter()
            .filter(|message| {
                message.metadata.semantic_kind == Some(MessageSemanticKind::ActualUserInput)
            })
            .count(),
        1,
        "recovery must not duplicate the original user message"
    );
    assert!(plan.messages.iter().any(|message| {
        message
            .content
            .to_string()
            .contains("previous work was interrupted")
    }));
    assert!(plan.messages.iter().any(|message| {
            matches!(
                &message.content,
                MessageContent::ToolResult {
                    tool_id,
                    is_error: false,
                    result,
                    ..
                } if tool_id == "in-flight-recovery-call"
                    && result == &json!("Tool execution was still in progress while resuming context; no result was available.")
            )
        }));

    let duplicate =
        reopen_interrupted_turn_for_test(&manager, &session.session_id, &plan.turn_id, 0)
            .await
            .expect_err("stale generation must be rejected");
    assert!(duplicate.to_string().contains("generation"), "{duplicate}");

    manager
        .cancel_dialog_turn(&session.session_id, &plan.turn_id)
        .await
        .expect("ordinary cancellation should settle recovered work");
    let persisted = persistence_manager
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .expect("turn should load")
        .expect("turn should exist");
    assert!(
        persisted.recovery.is_none(),
        "ordinary cancellation must remove the recoverable state"
    );
}

#[tokio::test]
async fn interrupted_turn_recovery_does_not_narrow_its_frozen_permission_mode() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Recovery permission".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                permission_mode: Some(PermissionMode::FullAccess),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "finish with the original permission".to_string(),
            Some("turn-permission".to_string()),
            None,
            Some(serde_json::json!({
                "resolved_permission_mode": "full_access",
                "runtime_resolved_model_id": "model-original",
                "runtime_model_binding_fingerprint": interrupted_turn_test_model_fingerprint(),
                "runtime_reasoning_preset": null,
                "runtime_reasoning_selection": null,
                "runtime_reasoning_fingerprint": interrupted_turn_test_reasoning_fingerprint(None)
            })),
        )
        .await
        .expect("turn should start");
    manager
        .mark_dialog_turn_interrupted(&session.session_id, &turn_id)
        .await
        .expect("turn should become interrupted");
    manager
        .update_session_state_for_turn_if_processing(
            &session.session_id,
            &turn_id,
            SessionState::Idle,
        )
        .await
        .expect("session should settle idle");
    manager
        .update_session_permission_mode(&session.session_id, Some(PermissionMode::Ask))
        .await
        .expect("session permission should change after interruption");

    let plan = reopen_interrupted_turn_for_test(&manager, &session.session_id, &turn_id, 0)
        .await
        .expect("turn should keep its original permission contract");
    assert_eq!(plan.resolved_permission_mode, PermissionMode::FullAccess);
}

#[tokio::test]
async fn interrupted_turn_recovery_rejects_a_changed_reasoning_preset() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Recovery reasoning preset".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                reasoning_preset: Some("high".to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = manager
            .start_dialog_turn(
                &session.session_id,
                "Standard".to_string(),
                "continue with one reasoning contract".to_string(),
                Some("turn-reasoning".to_string()),
                None,
                Some(serde_json::json!({
                    "resolved_permission_mode": "ask",
                    "runtime_resolved_model_id": "model-original",
                    "runtime_model_binding_fingerprint": interrupted_turn_test_model_fingerprint(),
                    "runtime_reasoning_preset": "high",
                    "runtime_reasoning_selection": "high",
                    "runtime_reasoning_fingerprint": interrupted_turn_test_reasoning_fingerprint(Some("high"))
                })),
            )
            .await
            .expect("turn should start");
    manager
        .mark_dialog_turn_interrupted(&session.session_id, &turn_id)
        .await
        .expect("turn should become interrupted");
    manager
        .update_session_state_for_turn_if_processing(
            &session.session_id,
            &turn_id,
            SessionState::Idle,
        )
        .await
        .expect("session should settle idle");
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("session should remain loaded")
        .config
        .reasoning_preset = Some("low".to_string());

    let error = reopen_interrupted_turn_for_test(&manager, &session.session_id, &turn_id, 0)
        .await
        .expect_err("same Turn must not silently change reasoning preset");

    assert!(error.to_string().contains("reasoning preset"), "{error}");
}

#[tokio::test]
async fn interrupted_turn_recovery_rejects_a_changed_auto_reasoning_default() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Recovery auto reasoning".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                model_id: Some("model-original".to_string()),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = manager
            .start_dialog_turn(
                &session.session_id,
                "Standard".to_string(),
                "continue with the original auto reasoning default".to_string(),
                Some("turn-auto-reasoning".to_string()),
                None,
                Some(serde_json::json!({
                    "resolved_permission_mode": "ask",
                    "runtime_resolved_model_id": "model-original",
                    "runtime_model_binding_fingerprint": service_model_runtime_binding_fingerprint(
                        &reasoning_model_with_default("model-original", "low")
                    ),
                    "runtime_reasoning_preset": "high",
                    "runtime_reasoning_selection": null,
                    "runtime_reasoning_fingerprint": interrupted_turn_test_auto_reasoning_fingerprint("high")
                })),
            )
            .await
            .expect("turn should start");
    manager
        .mark_dialog_turn_interrupted(&session.session_id, &turn_id)
        .await
        .expect("turn should become interrupted");
    manager
        .update_session_state_for_turn_if_processing(
            &session.session_id,
            &turn_id,
            SessionState::Idle,
        )
        .await
        .expect("session should settle idle");

    let error = TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            test_ai_config(vec![reasoning_model_with_default("model-original", "low")]),
            manager.reopen_interrupted_dialog_turn(&session.session_id, &turn_id, 0),
        )
        .await
        .expect_err("Auto must not resolve to a different preset for the same Turn");

    assert!(error.to_string().contains("reasoning contract"), "{error}");
}

#[tokio::test]
async fn interrupted_turn_recovery_rejects_a_changed_model_binding() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Recovery model binding".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                model_id: Some("model-original".to_string()),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "continue with the original model binding".to_string(),
            Some("turn-model-binding".to_string()),
            None,
            Some(serde_json::json!({
                "resolved_permission_mode": "ask",
                "runtime_resolved_model_id": "model-original",
                "runtime_model_binding_fingerprint": interrupted_turn_test_model_fingerprint(),
                "runtime_reasoning_preset": null,
                "runtime_reasoning_selection": null,
                "runtime_reasoning_fingerprint": interrupted_turn_test_reasoning_fingerprint(None)
            })),
        )
        .await
        .expect("turn should start");
    manager
        .mark_dialog_turn_interrupted(&session.session_id, &turn_id)
        .await
        .expect("turn should become interrupted");
    manager
        .update_session_state_for_turn_if_processing(
            &session.session_id,
            &turn_id,
            SessionState::Idle,
        )
        .await
        .expect("session should settle idle");

    let mut changed_model = configured_reasoning_model("model-original");
    changed_model.model_name = "provider-model-v2".to_string();
    let error = TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            test_ai_config(vec![changed_model]),
            manager.reopen_interrupted_dialog_turn(&session.session_id, &turn_id, 0),
        )
        .await
        .expect_err("same model ID must not silently change its runtime binding");

    assert!(error.to_string().contains("model binding"), "{error}");
}

#[tokio::test]
async fn recovery_persistence_failure_keeps_memory_and_disk_interrupted() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Recovery rollback".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "finish the task".to_string(),
            Some("turn-rollback".to_string()),
            None,
            Some(serde_json::json!({
                "resolved_permission_mode": "ask",
                "runtime_resolved_model_id": "model-original",
                "runtime_model_binding_fingerprint": interrupted_turn_test_model_fingerprint(),
                "runtime_reasoning_preset": null,
                "runtime_reasoning_selection": null,
                "runtime_reasoning_fingerprint": interrupted_turn_test_reasoning_fingerprint(None)
            })),
        )
        .await
        .expect("turn should start");
    manager
        .mark_dialog_turn_interrupted(&session.session_id, &turn_id)
        .await
        .expect("turn should become interrupted");
    manager
        .update_session_state_for_turn_if_processing(
            &session.session_id,
            &turn_id,
            SessionState::Idle,
        )
        .await
        .expect("session should settle idle");

    persistence_manager.fail_next_session_state_write_for_test(&session.session_id);
    reopen_interrupted_turn_for_test(&manager, &session.session_id, &turn_id, 0)
        .await
        .expect_err("injected session write must reject recovery");

    let active = manager
        .get_session(&session.session_id)
        .expect("session should remain active");
    assert!(matches!(active.state, SessionState::Idle));
    let persisted_session = persistence_manager
        .load_session(workspace.path(), &session.session_id)
        .await
        .expect("session should reload");
    assert!(matches!(persisted_session.state, SessionState::Idle));
    let persisted_turn = persistence_manager
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .expect("turn should load")
        .expect("turn should exist");
    assert_eq!(persisted_turn.status, TurnStatus::Cancelled);
    assert_eq!(
        persisted_turn
            .recovery
            .as_ref()
            .map(|recovery| recovery.status),
        Some(DialogTurnRecoveryStatus::Interrupted),
    );

    let plan = reopen_interrupted_turn_for_test(&manager, &session.session_id, &turn_id, 0)
        .await
        .expect("retry should reopen after rollback");
    persistence_manager.fail_next_dialog_turn_write_for_test(&session.session_id);
    manager
        .complete_recovered_dialog_turn(
            &session.session_id,
            &turn_id,
            plan.execution_generation,
            "completion that cannot persist".to_string(),
            &[],
            TurnStats {
                total_rounds: 1,
                total_tools: 0,
                total_tokens: 0,
                duration_ms: 1,
            },
            Some("complete".to_string()),
            Some(true),
        )
        .await
        .expect_err("injected recovered completion write must fail");
    manager
        .mark_dialog_turn_interrupted(&session.session_id, &turn_id)
        .await
        .expect("failed recovered completion should return to interruption");
    manager
        .update_session_state_for_turn_if_processing(
            &session.session_id,
            &turn_id,
            SessionState::Idle,
        )
        .await
        .expect("fallback interruption should settle idle");
    let persisted_turn = persistence_manager
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .expect("turn should load")
        .expect("turn should exist");
    assert_eq!(persisted_turn.status, TurnStatus::Cancelled);
    assert_eq!(
        persisted_turn.recovery_epoch,
        Some(plan.execution_generation)
    );
    assert_eq!(
        persisted_turn
            .recovery
            .as_ref()
            .map(|recovery| (recovery.status, recovery.execution_generation)),
        Some((
            DialogTurnRecoveryStatus::Interrupted,
            plan.execution_generation
        )),
    );
}

fn reasoning_model_with_default(id: &str, default_preset: &str) -> ServiceAIModelConfig {
    ServiceAIModelConfig {
        id: id.to_string(),
        name: id.to_string(),
        provider: "openai".to_string(),
        base_url: "https://example.test/v1".to_string(),
        model_name: id.to_string(),
        enabled: true,
        reasoning: Some(ReasoningConfig {
            catalog: ReasoningCatalogBinding::Disabled,
            default_preset: Some(default_preset.to_string()),
            presets: ["low", "high"]
                .into_iter()
                .map(|preset_id| ReasoningPreset {
                    id: preset_id.to_string(),
                    actions: vec![ReasoningPresetAction::Effort {
                        value: preset_id.to_string(),
                    }],
                    ..Default::default()
                })
                .collect(),
        }),
        ..Default::default()
    }
}
