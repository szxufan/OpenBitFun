use super::super::{test_support::test_coordinator, ConversationCoordinator, TEST_BTW_AI_CONFIG};
use crate::agentic::core::{SessionConfig, SessionModelBindingPolicy};
use crate::agentic::session::TEST_MODEL_RESOLUTION_AI_CONFIG;
use crate::service::config::types::{AIConfig, AIModelEntry, ModelCapability};
use openbitfun_core_types::model_pool::ModelSelection;
use openbitfun_runtime_ports::AgentSessionModelTargetSelection;
use serde_json::json;

fn ai() -> AIConfig {
    AIConfig {
        models: vec![AIModelEntry {
            id: "side-model".into(),
            enabled: true,
            capabilities: vec![ModelCapability::TextChat],
            ..Default::default()
        }],
        btw: json!({"model_selection":{"kind":"model","model_ref":"side-model"}}),
        ..Default::default()
    }
}

#[test]
fn btw_defaults_preserve_inheritance_and_use_the_selected_models_reasoning_default() {
    assert!(
        ConversationCoordinator::btw_default_model_target(&AIConfig::default())
            .unwrap()
            .is_none()
    );
    let target = ConversationCoordinator::btw_default_model_target(&ai())
        .unwrap()
        .unwrap();
    assert_eq!(
        target.selection,
        ModelSelection::Model {
            model_ref: "side-model".into()
        }
    );
    assert!(target.reasoning_preset.is_none());

    let mut config = ai();
    config.model_pools.pools.push(json!({"id":"side-pool","name":"Side","strategy":"balanced","member_model_ids":["side-model"]}));
    config.btw = json!({"model_selection":{"kind":"pool","pool_id":"side-pool"}});
    assert_eq!(
        ConversationCoordinator::btw_default_model_target(&config)
            .unwrap()
            .unwrap()
            .selection,
        ModelSelection::Pool {
            pool_id: "side-pool".into()
        }
    );
    config.model_pools.pools.clear();
    assert!(ConversationCoordinator::btw_default_model_target(&config).is_err());
}

#[tokio::test]
async fn btw_default_overrides_parent_pool_without_replacing_context_or_remote_identity() {
    let (coordinator, manager) = test_coordinator();
    let workspace = crate::service::workspace::legacy_compat::register_remote_fixture(
        &format!("/srv/btw-model/{}", uuid::Uuid::new_v4()),
        "ssh-model-test",
        "example.test",
    )
    .await;
    let parent = manager
        .create_session(
            "Parent".into(),
            "Standard".into(),
            SessionConfig {
                workspace_id: Some(workspace.id.clone()),
                workspace_path: Some(workspace.root_path.to_string_lossy().into_owned()),
                model_pool_id: Some("parent-pool".into()),
                model_binding_policy: SessionModelBindingPolicy::Pool,
                reasoning_preset: Some("high".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    manager
        .replace_context_messages(
            &parent.session_id,
            vec![crate::agentic::core::Message::user("Parent context".into())],
        )
        .await;
    TEST_BTW_AI_CONFIG
        .scope(
            ai(),
            TEST_MODEL_RESOLUTION_AI_CONFIG.scope(ai(), async {
                let target = coordinator
                    .resolve_btw_initial_model_target("child", None, None, None)
                    .await
                    .unwrap()
                    .unwrap();
                let child = coordinator
                    .ensure_btw_session(&parent.session_id, "child", None, "question", None, None)
                    .await
                    .unwrap();
                let updated = manager
                    .update_session_model_target_with_reasoning(
                        &child.session_id,
                        &target.selection,
                        target.reasoning_preset.as_deref(),
                    )
                    .await
                    .unwrap();
                assert_eq!(updated.config.model_id.as_deref(), Some("side-model"));
                assert!(updated.config.model_pool_id.is_none());
                assert!(updated.config.reasoning_preset.is_none());
                assert_eq!(updated.config.workspace_id, parent.config.workspace_id);
                assert_eq!(
                    updated.config.remote_connection_id,
                    parent.config.remote_connection_id
                );
                assert_eq!(
                    manager.get_context_messages("child").await.unwrap().len(),
                    1
                );
                manager
                    .start_dialog_turn(
                        "child",
                        "Standard".into(),
                        "Question".into(),
                        None,
                        None,
                        None,
                    )
                    .await
                    .unwrap();
                // A changed/invalid global default is irrelevant once a question exists.
                let mut invalid = ai();
                invalid.btw = json!({"model_selection":{"kind":"future"}});
                TEST_BTW_AI_CONFIG
                    .scope(invalid, async {
                        assert!(coordinator
                            .resolve_btw_initial_model_target("child", None, None, None)
                            .await
                            .unwrap()
                            .is_none());
                        assert!(coordinator
                            .resolve_btw_initial_model_target("new-child", None, None, None)
                            .await
                            .is_err());
                        let explicit = AgentSessionModelTargetSelection {
                            selection: ModelSelection::Model {
                                model_ref: "primary".into(),
                            },
                            reasoning_preset: Some("high".into()),
                        };
                        let resolved = coordinator
                            .resolve_btw_initial_model_target(
                                "new-child",
                                None,
                                None,
                                Some(explicit),
                            )
                            .await
                            .unwrap()
                            .unwrap();
                        assert_eq!(resolved.reasoning_preset.as_deref(), Some("high"));
                        assert!(coordinator
                            .resolve_btw_initial_model_target("new-child", Some("fast"), None, None)
                            .await
                            .unwrap()
                            .is_none());
                    })
                    .await;
            }),
        )
        .await;
}
