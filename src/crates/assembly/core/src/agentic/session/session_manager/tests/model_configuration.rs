use super::super::{SessionManager, TEST_MODEL_RESOLUTION_AI_CONFIG};
#[cfg(feature = "model-catalog")]
use super::support::configured_reasoning_model;
use super::support::{test_ai_config, test_manager, test_model, TestWorkspace};
use crate::agentic::core::{Session, SessionConfig};
use crate::agentic::persistence::PersistenceManager;
use crate::service::session::SessionKind;
#[cfg(feature = "model-catalog")]
use openbitfun_core_types::{ReasoningCatalogBinding, ReasoningConfig};
use std::sync::Arc;

#[cfg(feature = "model-catalog")]
#[tokio::test]
async fn reasoning_preset_normalization_uses_the_updated_model() {
    let mut ai_config = test_ai_config(vec![
        configured_reasoning_model("model-a"),
        test_model("model-b", 128_000),
    ]);
    let mut session = Session::new_with_id(
        "reasoning-selection".to_string(),
        "Reasoning selection".to_string(),
        "Standard".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            model_id: Some("model-a".to_string()),
            reasoning_preset: Some("high".to_string()),
            ..Default::default()
        },
    );

    assert_eq!(
        SessionManager::normalize_session_reasoning_preset(&session, &ai_config).await,
        Some("high".to_string())
    );
    // model-b carries no explicit reasoning config; disable its catalog so
    // the switched session must not inherit model-a's preset.
    ai_config
        .models
        .iter_mut()
        .find(|model| model.id == "model-b")
        .unwrap()
        .reasoning = Some(ReasoningConfig {
        catalog: ReasoningCatalogBinding::Disabled,
        ..Default::default()
    });
    session.config.model_id = Some("model-b".to_string());
    assert_eq!(
        SessionManager::normalize_session_reasoning_preset(&session, &ai_config).await,
        None
    );
}

#[cfg(feature = "model-catalog")]
#[tokio::test]
async fn reasoning_preset_reconciliation_persists_auto_state() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Stale reasoning preset".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                model_id: Some("model-b".to_string()),
                reasoning_preset: Some("obsolete".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    // The test config service is intentionally absent, so seed the legacy
    // invalid state that reconciliation must canonicalize.
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("session remains loaded")
        .config
        .reasoning_preset = Some("obsolete".to_string());
    let seeded = manager
        .get_session(&session.session_id)
        .expect("seeded session");
    persistence_manager
        .save_session(workspace.path(), &seeded)
        .await
        .expect("seeded preset should persist");
    let ai_config = test_ai_config(vec![test_model("model-b", 128_000)]);

    manager
        .reconcile_session_reasoning_preset_locked(&session.session_id, &ai_config, "test")
        .await
        .expect("reconciliation should succeed");

    assert!(manager
        .get_session(&session.session_id)
        .expect("session remains loaded")
        .config
        .reasoning_preset
        .is_none());
    assert!(persistence_manager
        .load_session(workspace.path(), &session.session_id)
        .await
        .expect("session should reload")
        .config
        .reasoning_preset
        .is_none());
}

#[tokio::test]
async fn session_creation_persists_resolved_model_context_window() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let ai_config = test_ai_config(vec![test_model("deepseek-v4-flash", 200_000)]);

    let session = TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(ai_config, async {
            manager
                .create_session(
                    "Remote session".to_string(),
                    "Standard".to_string(),
                    SessionConfig {
                        tool_presentation_mode: Default::default(),
                        workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                        model_id: Some("deepseek-v4-flash".to_string()),
                        max_context_tokens: 128_128,
                        ..Default::default()
                    },
                )
                .await
        })
        .await
        .expect("session should create");

    assert_eq!(session.config.max_context_tokens, 200_000);
    let persisted = persistence_manager
        .load_session(workspace.path(), &session.session_id)
        .await
        .expect("persisted session should load");
    assert_eq!(persisted.config.max_context_tokens, 200_000);
}

#[test]
fn sync_session_context_window_refreshes_stale_explicit_model_window() {
    let ai_config = test_ai_config(vec![test_model("deepseek-v4-pro", 1_000_000)]);

    let mut session = Session::new_with_id(
        "session-804".to_string(),
        "DeepSeek session".to_string(),
        "Standard".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            model_id: Some("deepseek-v4-pro".to_string()),
            max_context_tokens: 256_000,
            ..Default::default()
        },
    );

    let resolved =
        SessionManager::sync_session_context_window_from_ai_config(&mut session, &ai_config);

    assert_eq!(resolved, Some(1_000_000));
    assert_eq!(session.config.max_context_tokens, 1_000_000);
}

#[test]
fn sync_session_context_window_resolves_missing_selection_through_mode_default_then_primary() {
    let mut ai_config = test_ai_config(vec![
        test_model("primary-model", 512_000),
        test_model("agent-model", 1_000_000),
    ]);
    ai_config.default_models.primary = Some("primary-model".to_string());
    ai_config
        .set_model_selections(
            openbitfun_config_contracts::model_pool::AgentModelSelections {
                mode: openbitfun_core_types::model_pool::ModelSelection::Model {
                    model_ref: "agent-model".into(),
                },
                ..Default::default()
            },
        )
        .unwrap();

    let mut session = Session::new_with_id(
        "session-default".to_string(),
        "Default session".to_string(),
        "Standard".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            model_id: None,
            max_context_tokens: 256_000,
            ..Default::default()
        },
    );

    let resolved =
        SessionManager::sync_session_context_window_from_ai_config(&mut session, &ai_config);

    assert_eq!(resolved, Some(1_000_000));
    assert_eq!(session.config.max_context_tokens, 1_000_000);

    ai_config
        .set_model_selections(
            openbitfun_config_contracts::model_pool::AgentModelSelections {
                mode: openbitfun_core_types::model_pool::ModelSelection::Model {
                    model_ref: "primary".into(),
                },
                ..Default::default()
            },
        )
        .unwrap();
    session.config.max_context_tokens = 256_000;

    let resolved =
        SessionManager::sync_session_context_window_from_ai_config(&mut session, &ai_config);

    assert_eq!(resolved, Some(512_000));
    assert_eq!(session.config.max_context_tokens, 512_000);
}

#[test]
fn sync_session_context_window_resolves_missing_subagent_selection_through_primary() {
    let mut ai_config = test_ai_config(vec![
        test_model("primary-model", 512_000),
        test_model("mode-model", 1_000_000),
    ]);
    ai_config.default_models.primary = Some("primary-model".to_string());
    ai_config
        .set_model_selections(
            openbitfun_config_contracts::model_pool::AgentModelSelections {
                mode: openbitfun_core_types::model_pool::ModelSelection::Model {
                    model_ref: "mode-model".into(),
                },
                ..Default::default()
            },
        )
        .unwrap();

    let mut session = Session::new_with_id(
        "subagent-default".to_string(),
        "Default subagent".to_string(),
        "Explore".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            model_id: None,
            max_context_tokens: 256_000,
            ..Default::default()
        },
    );
    session.kind = SessionKind::Subagent;

    let resolved =
        SessionManager::sync_session_context_window_from_ai_config(&mut session, &ai_config);

    assert_eq!(resolved, Some(512_000));
    assert_eq!(session.config.max_context_tokens, 512_000);
}
