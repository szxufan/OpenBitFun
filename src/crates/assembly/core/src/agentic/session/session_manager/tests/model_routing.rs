use super::super::{
    should_apply_session_model_fallback, SessionManager, TEST_MODEL_RESOLUTION_AI_CONFIG,
};
use super::support::{
    configured_reasoning_model, test_ai_config, test_manager, test_model, TestWorkspace,
};
use crate::agentic::core::{Session, SessionConfig, SessionModelBindingPolicy};
use crate::agentic::persistence::PersistenceManager;
use crate::infrastructure::PathManager;
use crate::service::config::types::AIModelConfig as ServiceAIModelConfig;
use crate::service::config::{ConfigManagerSettings, ConfigService};
use crate::service::session::SessionKind;
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn runtime_model_is_visible_to_turn_admission_config() {
    let dir = tempfile::tempdir().expect("temporary config directory");
    let config = ConfigService::with_settings(ConfigManagerSettings {
        path_manager: Some(Arc::new(PathManager::with_user_root_for_tests(
            dir.path().join("runtime-turn-admission"),
        ))),
        auto_save: true,
        backup_count: 0,
    })
    .await
    .expect("test ConfigService");
    config
        .install_runtime_ai_model(ServiceAIModelConfig {
            id: "sdk:openai:fixture".to_string(),
            name: "SDK fixture".to_string(),
            provider: "openai".to_string(),
            model_name: "fixture-model".to_string(),
            base_url: "http://127.0.0.1:43123/v1".to_string(),
            api_key: "fixture-secret".to_string(),
            enabled: true,
            ..ServiceAIModelConfig::default()
        })
        .await
        .unwrap();

    let ai_config = SessionManager::load_effective_ai_config_from_service(&config)
        .await
        .expect("turn admission should see the runtime model");
    assert_eq!(
        ai_config.resolve_model_reference("sdk:openai:fixture"),
        Some("sdk:openai:fixture".to_string())
    );
}

#[test]
fn invalidated_model_fallback_preserves_approved_external_generation_binding() {
    let invalidated = HashSet::from(["removed-model"]);

    assert!(should_apply_session_model_fallback(
        SessionModelBindingPolicy::Mutable,
        "removed-model",
        &invalidated,
    ));
    assert!(!should_apply_session_model_fallback(
        SessionModelBindingPolicy::ApprovedImmutable,
        "removed-model",
        &invalidated,
    ));
    assert!(!should_apply_session_model_fallback(
        SessionModelBindingPolicy::Mutable,
        "active-model",
        &invalidated,
    ));
}

#[tokio::test]
async fn model_routing_preserves_strategy_when_only_reasoning_changes() {
    let workspace = TestWorkspace::new();
    let persistence = Arc::new(PersistenceManager::new(workspace.path_manager()).unwrap());
    let manager = test_manager(persistence.clone());
    let id = Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        id.clone(),
        "Smart session".into(),
        "Standard".into(),
        SessionConfig {
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
            model_id: Some("primary".into()),
            model_routing: Some("smart".into()),
            reasoning_preset: Some("high".into()),
            ..Default::default()
        },
    );
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    let updated = manager
        .update_session_model_selection_in_storage(
            Some(workspace.path()),
            &id,
            "primary",
            None,
            false,
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(updated.config.model_routing.as_deref(), Some("smart"));
    assert!(updated.config.reasoning_preset.is_none());
    let persisted = persistence
        .load_session_header(workspace.path(), &id)
        .await
        .unwrap();
    assert_eq!(persisted.config.model_routing.as_deref(), Some("smart"));
    assert_eq!(persisted.config.model_id.as_deref(), Some("primary"));
    assert!(persisted.config.reasoning_preset.is_none());
    assert!(manager.get_session(&id).is_none());
}

#[tokio::test]
async fn smart_release_gate_new_sessions_ignore_saved_smart_defaults() {
    let workspace = TestWorkspace::new();
    let persistence = Arc::new(PersistenceManager::new(workspace.path_manager()).unwrap());
    let manager = test_manager(persistence);
    let mut ai = crate::service::config::types::AIConfig::default();
    ai.model_routing.default_mode = Some("smart".into());
    ai.model_routing.summary_enabled = false;
    let session = TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            ai,
            manager.create_session(
                "Release gate fixture".into(),
                "Standard".into(),
                SessionConfig {
                    workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                    ..Default::default()
                },
            ),
        )
        .await
        .unwrap();
    assert_eq!(session.config.model_routing.as_deref(), Some("manual"));
}

#[tokio::test]
async fn smart_release_gate_reconcile_preserves_old_smart_but_keeps_manual_fallback() {
    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let persistence = Arc::new(PersistenceManager::new(paths.clone()).unwrap());
    let manager = test_manager(persistence);
    let mut ai = crate::service::config::types::AIConfig::default();
    ai.providers
        .push(crate::service::config::types::AIProviderConfig {
            id: "prov".into(),
            name: "Provider prov".into(),
            provider: "openai".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "key".into(),
            enabled: true,
            ..Default::default()
        });
    for id in ["removed-model", "fallback-model"] {
        ai.models.push(crate::service::config::types::AIModelEntry {
            id: id.into(),
            provider_id: "prov".into(),
            model_name: id.into(),
            enabled: true,
            ..Default::default()
        });
    }
    ai.default_models.primary = Some("fallback-model".into());
    let (retained, manual) = TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(ai.clone(), async {
            let mut sessions = Vec::new();
            for mode in ["smart", "manual"] {
                sessions.push(
                    manager
                        .create_session(
                            mode.into(),
                            "Standard".into(),
                            SessionConfig {
                                model_id: Some("removed-model".into()),
                                model_routing: Some(mode.into()),
                                workspace_path: Some(
                                    workspace.path().to_string_lossy().into_owned(),
                                ),
                                ..Default::default()
                            },
                        )
                        .await
                        .unwrap(),
                );
            }
            (sessions.remove(0), sessions.remove(0))
        })
        .await;
    let state_path = paths
        .project_sessions_dir(workspace.path())
        .join(&retained.session_id)
        .join("state.json");
    let bytes = std::fs::read(&state_path).unwrap();
    ai.models.retain(|model| model.id != "removed-model");
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            ai,
            manager.apply_fallback_to_invalidated_session_models(
                &["removed-model".into()],
                "release_gate_fixture",
            ),
        )
        .await;
    let retained = manager.get_session(&retained.session_id).unwrap();
    assert_eq!(retained.config.model_routing.as_deref(), Some("smart"));
    assert_eq!(retained.config.model_id.as_deref(), Some("removed-model"));
    assert_eq!(std::fs::read(&state_path).unwrap(), bytes);
    let manual = manager.get_session(&manual.session_id).unwrap();
    assert_eq!(manual.config.model_routing.as_deref(), Some("manual"));
    assert_eq!(manual.config.model_id.as_deref(), Some("primary"));
}

#[tokio::test]
async fn smart_release_gate_restore_preserves_old_smart_model_reference() {
    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let persistence = Arc::new(PersistenceManager::new(paths.clone()).unwrap());
    let manager = test_manager(persistence.clone());
    let id = Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        id.clone(),
        "Retained smart session".into(),
        "Standard".into(),
        SessionConfig {
            model_id: Some("removed-model".into()),
            model_routing: Some("smart".into()),
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
            ..Default::default()
        },
    );
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    let summary_path = paths
        .project_sessions_dir(workspace.path())
        .join(&id)
        .join("retained-router-summary.json");
    std::fs::write(&summary_path, b"Retained historical summary evidence").unwrap();
    let mut ai = crate::service::config::types::AIConfig::default();
    ai.providers
        .push(crate::service::config::types::AIProviderConfig {
            id: "fallback-model".to_string() + "-provider",
            name: "Provider".into(),
            provider: "openai".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "key".into(),
            enabled: true,
            ..Default::default()
        });
    ai.models.push(crate::service::config::types::AIModelEntry {
        id: "fallback-model".into(),
        provider_id: "fallback-model".to_string() + "-provider",
        model_name: "fallback-model".into(),
        enabled: true,
        ..Default::default()
    });
    ai.default_models.primary = Some("fallback-model".into());
    let restored = TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(ai, manager.restore_session(workspace.path(), &id))
        .await
        .unwrap();
    assert_eq!(restored.config.model_routing.as_deref(), Some("smart"));
    assert_eq!(restored.config.model_id.as_deref(), Some("removed-model"));
    let persisted = persistence
        .load_session_header(workspace.path(), &id)
        .await
        .unwrap();
    assert_eq!(persisted.config.model_routing.as_deref(), Some("smart"));
    assert_eq!(persisted.config.model_id.as_deref(), Some("removed-model"));
    assert_eq!(
        std::fs::read(&summary_path).unwrap(),
        b"Retained historical summary evidence"
    );
}

#[tokio::test]
async fn smart_release_gate_rejects_public_smart_selection_without_changing_old_records() {
    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let persistence = Arc::new(PersistenceManager::new(paths.clone()).unwrap());
    let manager = test_manager(persistence.clone());
    let id = Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        id.clone(),
        "Retained smart session".into(),
        "Standard".into(),
        SessionConfig {
            model_id: Some("primary".into()),
            model_routing: Some("smart".into()),
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
            ..Default::default()
        },
    );
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    let directory = paths.project_sessions_dir(workspace.path()).join(&id);
    let state_path = directory.join("state.json");
    let summary_path = directory.join("retained-router-summary.json");
    std::fs::write(&summary_path, b"Retained historical summary evidence").unwrap();
    for loaded in [false, true] {
        if loaded {
            manager
                .restore_session(workspace.path(), &id)
                .await
                .unwrap();
            assert_eq!(
                manager
                    .get_session(&id)
                    .unwrap()
                    .config
                    .model_routing
                    .as_deref(),
                Some("smart")
            );
        }
        let bytes = std::fs::read(&state_path).unwrap();
        let error = manager
            .update_session_model_selection_in_storage_with_routing(
                if loaded { None } else { Some(workspace.path()) },
                &id,
                "primary",
                Some("high"),
                Some("smart"),
                false,
                |_| Ok(()),
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Smart mode is unavailable in this release"),
            "loaded={loaded}: {error}"
        );
        assert_eq!(std::fs::read(&state_path).unwrap(), bytes);
        assert_eq!(
            std::fs::read(&summary_path).unwrap(),
            b"Retained historical summary evidence"
        );
    }
}

#[tokio::test]
async fn smart_release_gate_legacy_explicit_model_change_returns_to_manual() {
    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let persistence = Arc::new(PersistenceManager::new(paths.clone()).unwrap());
    let manager = test_manager(persistence.clone());
    let id = Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        id.clone(),
        "Retained smart session".into(),
        "Standard".into(),
        SessionConfig {
            model_id: Some("primary".into()),
            model_routing: Some("smart".into()),
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
            ..Default::default()
        },
    );
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    let directory = paths.project_sessions_dir(workspace.path()).join(&id);
    let summary_path = directory.join("retained-router-summary.json");
    std::fs::write(&summary_path, b"Retained historical summary evidence").unwrap();
    let mut ai = crate::service::config::types::AIConfig::default();
    ai.providers
        .push(crate::service::config::types::AIProviderConfig {
            id: "chosen-model".to_string() + "-provider",
            name: "Provider".into(),
            provider: "openai".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "key".into(),
            enabled: true,
            ..Default::default()
        });
    ai.models.push(crate::service::config::types::AIModelEntry {
        id: "chosen-model".into(),
        provider_id: "chosen-model".to_string() + "-provider",
        model_name: "chosen-model".into(),
        enabled: true,
        ..Default::default()
    });
    let updated = TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            ai,
            manager.update_session_model_selection_in_storage(
                Some(workspace.path()),
                &id,
                "chosen-model",
                None,
                false,
                |_| Ok(()),
            ),
        )
        .await
        .unwrap();
    assert_eq!(updated.config.model_routing.as_deref(), Some("manual"));
    assert_eq!(updated.config.model_id.as_deref(), Some("chosen-model"));
    let persisted = persistence
        .load_session_header(workspace.path(), &id)
        .await
        .unwrap();
    assert_eq!(persisted.config.model_routing.as_deref(), Some("manual"));
    assert_eq!(persisted.config.model_id.as_deref(), Some("chosen-model"));
    assert_eq!(
        std::fs::read(&summary_path).unwrap(),
        b"Retained historical summary evidence"
    );
    assert!(manager.get_session(&id).is_none());
}

#[tokio::test]
async fn model_routing_explicit_typed_target_returns_to_manual_without_restoring_session() {
    let workspace = TestWorkspace::new();
    let persistence = Arc::new(PersistenceManager::new(workspace.path_manager()).unwrap());
    let manager = test_manager(persistence.clone());
    let id = Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        id.clone(),
        "Smart session".into(),
        "Standard".into(),
        SessionConfig {
            model_id: Some("primary".into()),
            model_routing: Some("smart".into()),
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
            ..Default::default()
        },
    );
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    let mut ai = crate::service::config::types::AIConfig::default();
    ai.providers
        .push(crate::service::config::types::AIProviderConfig {
            id: "chosen-model".to_string() + "-provider",
            name: "Provider".into(),
            provider: "openai".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "key".into(),
            enabled: true,
            ..Default::default()
        });
    ai.models.push(crate::service::config::types::AIModelEntry {
        id: "chosen-model".into(),
        provider_id: "chosen-model".to_string() + "-provider",
        model_name: "chosen-model".into(),
        enabled: true,
        ..Default::default()
    });
    let target = openbitfun_core_types::model_pool::ModelSelection::Model {
        model_ref: "chosen-model".into(),
    };
    let updated = TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            ai,
            manager.update_session_selection_in_storage(
                Some(workspace.path()),
                &id,
                "chosen-model",
                None,
                None,
                false,
                Some(&target),
                |_| Ok(()),
            ),
        )
        .await
        .unwrap();
    assert_eq!(updated.config.model_routing.as_deref(), Some("manual"));
    assert_eq!(updated.config.model_id.as_deref(), Some("chosen-model"));
    let saved = persistence
        .load_session_header(workspace.path(), &id)
        .await
        .unwrap();
    assert_eq!(saved.config.model_routing.as_deref(), Some("manual"));
    assert_eq!(saved.config.model_id.as_deref(), Some("chosen-model"));
    assert!(manager.get_session(&id).is_none());
}

#[tokio::test]
async fn restore_rewrites_retired_auto_model_selector_to_primary() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let session_id = uuid::Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        session_id.clone(),
        "Legacy model selector".to_string(),
        "Standard".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
            model_id: Some("auto".to_string()),
            ..Default::default()
        },
    );
    persistence_manager
        .save_session(workspace.path(), &session)
        .await
        .expect("legacy session fixture should persist");
    let manager = test_manager(persistence_manager.clone());
    let mut ai_config = test_ai_config(vec![test_model("primary-model", 512_000)]);
    ai_config.default_models.primary = Some("primary-model".to_string());

    let restored = TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            ai_config,
            manager.restore_session(workspace.path(), &session_id),
        )
        .await
        .expect("legacy session should restore");

    assert_eq!(restored.config.model_id.as_deref(), Some("primary"));
    assert_eq!(
        persistence_manager
            .load_session(workspace.path(), &session_id)
            .await
            .expect("rewritten session should persist")
            .config
            .model_id
            .as_deref(),
        Some("primary")
    );
}

#[tokio::test]
async fn btw_typed_model_choice_leaves_inherited_pool_without_mutating_parent() {
    use openbitfun_core_types::model_pool::{ModelPoolBinding, ModelSelection};
    let workspace = TestWorkspace::new();
    let manager = test_manager(Arc::new(
        PersistenceManager::new(workspace.path_manager()).unwrap(),
    ));
    let mut parent = Session::new(
        "Parent".into(),
        "Standard".into(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
            model_id: Some("model-a".into()),
            model_pool_id: Some("pool-a".into()),
            model_pool_binding: Some(ModelPoolBinding {
                pool_id: "pool-a".into(),
                model_id: "model-a".into(),
                configuration_fingerprint: "model-a-v1".into(),
                generation: 3,
            }),
            model_binding_policy: SessionModelBindingPolicy::Pool,
            ..Default::default()
        },
    );
    parent.kind = SessionKind::EphemeralChild;
    let snapshot =
        crate::agentic::fork_agent::ForkAgentContextSnapshot::from_parent_session(&parent, vec![])
            .unwrap();
    let mut child = Session::new(
        "Side question".into(),
        "Standard".into(),
        snapshot.build_child_session_config(None),
    );
    child.kind = SessionKind::EphemeralChild;
    assert_eq!(
        child.config.model_pool_binding,
        parent.config.model_pool_binding
    );
    let child_id = child.session_id.clone();
    manager
        .sessions
        .insert(parent.session_id.clone(), parent.clone());
    manager.sessions.insert(child_id.clone(), child.clone());
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            test_ai_config(vec![
                test_model("model-a", 128_000),
                configured_reasoning_model("model-b"),
            ]),
            async {
                assert!(manager
                    .update_session_model_selection(&child_id, "model-b", Some("high"))
                    .await
                    .is_err());
                assert!(manager
                    .update_session_model_target_with_reasoning(
                        &child_id,
                        &ModelSelection::Model {
                            model_ref: "missing".into()
                        },
                        Some("high")
                    )
                    .await
                    .is_err());
                let unchanged = manager.get_session(&child_id).unwrap();
                assert_eq!(
                    unchanged.config.model_pool_binding,
                    child.config.model_pool_binding
                );
                assert_eq!(
                    unchanged.config.model_pool_selection_revision,
                    child.config.model_pool_selection_revision
                );
                let selected = manager
                    .update_session_model_target_with_reasoning(
                        &child_id,
                        &ModelSelection::Model {
                            model_ref: "model-b".into(),
                        },
                        Some("high"),
                    )
                    .await
                    .unwrap();
                assert_eq!(selected.config.model_id.as_deref(), Some("model-b"));
                assert_eq!(selected.config.reasoning_preset.as_deref(), Some("high"));
                assert!(selected.config.model_pool_id.is_none());
                assert!(selected.config.model_pool_binding.is_none());
                assert_eq!(
                    selected.config.model_binding_policy,
                    SessionModelBindingPolicy::Mutable
                );
                assert_eq!(
                    manager
                        .get_session(&parent.session_id)
                        .unwrap()
                        .config
                        .model_pool_binding,
                    parent.config.model_pool_binding
                );
            },
        )
        .await;
}
