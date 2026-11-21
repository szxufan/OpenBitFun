use super::super::test_support::test_ai_config;
use super::super::test_support::test_coordinator;
use super::super::{
    apply_primary_agent_model_default, normalize_model_selection, resolve_subagent_model_selection,
    runtime_session_summary, snapshot_normal_session_model, SubagentExecutionRequest,
    TEST_AGENT_MODEL_DEFAULTS,
};
use crate::agentic::agents::ExternalSubagentModelBinding;
use crate::agentic::core::{
    Message, SessionConfig, SessionContinuationPolicy, SessionKind, SessionModelBindingPolicy,
};
use crate::agentic::session::TEST_MODEL_RESOLUTION_AI_CONFIG;
use crate::agentic::tools::pipeline::SubagentParentInfo;
use crate::service::config::types::{model_runtime_binding_fingerprint, AIConfig, AIModelConfig};
use crate::service::config::SubagentModelSelection;
use openbitfun_config_contracts::model_pool::{AgentModelSelection, AgentModelSelections};
use openbitfun_core_types::model_pool::ModelSelection;
use openbitfun_runtime_ports::{DelegationPolicy, PermissionRuntimeCeiling, SubagentContextMode};
use std::collections::HashMap;

#[test]
fn external_command_delegation_uses_the_resolved_primary_binding() {
    let source = include_str!("../../coordinator.rs").replace("\r\n", "\n");
    let delegation = source
        .split_once("pub(crate) fn start_external_subagent_delegation_turn(")
        .expect("external command delegation entry")
        .1
        .split_once("pub async fn start_dialog_turn_with_prepended_messages(")
        .expect("external command delegation boundary")
        .0;

    assert!(delegation.contains("Self::resolve_session_agent_for_turn("));
    assert!(delegation.contains("Some(&primary_runtime_agent_key)"));
    assert!(delegation.contains(".update_session_agent_binding("));
    assert!(!delegation.contains(".update_session_agent_type("));
    assert!(delegation
        .contains("let _primary_agent_generation_lease = primary_agent_generation_lease;"));
}

#[test]
fn external_primary_fixed_model_is_only_a_creation_default() {
    let fixed = ExternalSubagentModelBinding::Fixed {
        model_id: "provider/profile-model".to_string(),
        configuration_fingerprint: "fingerprint".to_string(),
    };

    let mut omitted = SessionConfig::default();
    apply_primary_agent_model_default(&mut omitted, Some(&fixed));
    assert_eq!(omitted.model_id.as_deref(), Some("provider/profile-model"));

    let mut defaulted = SessionConfig {
        tool_presentation_mode: Default::default(),
        model_id: Some("default".to_string()),
        ..SessionConfig::default()
    };
    apply_primary_agent_model_default(&mut defaulted, Some(&fixed));
    assert_eq!(
        defaulted.model_id.as_deref(),
        Some("provider/profile-model")
    );

    let mut legacy_auto = SessionConfig {
        tool_presentation_mode: Default::default(),
        model_id: Some("auto".to_string()),
        ..SessionConfig::default()
    };
    apply_primary_agent_model_default(&mut legacy_auto, Some(&fixed));
    assert_eq!(
        legacy_auto.model_id.as_deref(),
        Some("provider/profile-model")
    );

    let mut explicit = SessionConfig {
        tool_presentation_mode: Default::default(),
        model_id: Some("provider/user-model".to_string()),
        ..SessionConfig::default()
    };
    apply_primary_agent_model_default(&mut explicit, Some(&fixed));
    assert_eq!(explicit.model_id.as_deref(), Some("provider/user-model"));

    let mut inherited = SessionConfig::default();
    apply_primary_agent_model_default(
        &mut inherited,
        Some(&ExternalSubagentModelBinding::InheritParent),
    );
    assert_eq!(inherited.model_id, None);
}

#[tokio::test]
async fn retired_auto_model_input_is_canonicalized_to_primary() {
    assert_eq!(
        normalize_model_selection(" auto ")
            .await
            .expect("legacy selector should remain upgrade-compatible"),
        "primary"
    );
}

#[test]
fn model_pool_default_snapshot_and_subagent_inheritance_keep_intent() {
    use openbitfun_config_contracts::model_pool::AgentModelSelection;
    use openbitfun_core_types::model_pool::ModelSelection;
    let pool = ModelSelection::Pool {
        pool_id: "workers".into(),
    };
    let mut defaults = AgentModelSelections::default();
    defaults.mode = pool.clone();
    let mut config = SessionConfig::default();
    snapshot_normal_session_model(&mut config, &defaults).unwrap();
    assert_eq!(config.model_pool_id.as_deref(), Some("workers"));
    assert!(config.model_id.is_none());
    let target = super::super::resolve_subagent_model_target(
        None,
        &AgentModelSelection::Inherit,
        Some(pool.clone()),
    )
    .unwrap();
    assert_eq!(target, pool);
    super::super::apply_session_model_target(&mut config, target);
    assert!(
        config.model_pool_binding.is_none(),
        "a child must acquire its own binding"
    );
    let fixed = super::super::resolve_subagent_model_target(
        Some("a"),
        &AgentModelSelection::Pool {
            pool_id: "workers".into(),
        },
        Some(pool),
    )
    .unwrap();
    super::super::apply_session_model_target(&mut config, fixed);
    assert_eq!(config.model_id.as_deref(), Some("a"));
    assert!(config.model_pool_id.is_none());
}

#[test]
fn retired_auto_model_input_is_not_snapshotted_into_new_sessions() {
    let mut config = SessionConfig {
        tool_presentation_mode: Default::default(),
        model_id: Some("auto".to_string()),
        ..SessionConfig::default()
    };
    let defaults = AgentModelSelections {
        mode: openbitfun_core_types::model_pool::ModelSelection::Model {
            model_ref: "configured-default".into(),
        },
        ..AgentModelSelections::default()
    };

    snapshot_normal_session_model(&mut config, &defaults).unwrap();

    assert_eq!(config.model_id.as_deref(), Some("configured-default"));
}

#[test]
fn runtime_session_list_preserves_the_runtime_owned_model_selector() {
    let summary = runtime_session_summary(openbitfun_agent_runtime::session::SessionSummary {
        session_id: "session".to_string(),
        session_name: "Session".to_string(),
        agent_type: "Standard".to_string(),
        model_id: Some("fast".to_string()),
        reasoning_preset: Some("high".to_string()),
        last_user_dialog_agent_type: None,
        last_submitted_agent_type: None,
        created_by: None,
        kind: SessionKind::Standard,
        turn_count: 0,
        created_at: std::time::UNIX_EPOCH,
        last_activity_at: std::time::UNIX_EPOCH,
        state: openbitfun_agent_runtime::session_state::SessionState::Idle,
    });

    assert_eq!(summary.model_id.as_deref(), Some("fast"));
}

#[tokio::test]
async fn normal_sessions_keep_the_mode_default_snapshotted_at_creation() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-normal-session-model-snapshot-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&workspace_path).expect("workspace dir should exist");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace_path);
    let workspace_path_string = workspace_path.to_string_lossy().into_owned();

    let first = TEST_AGENT_MODEL_DEFAULTS
        .scope(
            AgentModelSelections {
                mode: openbitfun_core_types::model_pool::ModelSelection::Model {
                    model_ref: "model-a".into(),
                },
                ..Default::default()
            },
            coordinator.create_session_with_workspace(
                None,
                "First".to_string(),
                "Standard".to_string(),
                SessionConfig {
                    tool_presentation_mode: Default::default(),
                    workspace_path: Some(workspace_path_string.clone()),
                    ..Default::default()
                },
                workspace_path_string.clone(),
            ),
        )
        .await
        .expect("first normal session should be created");

    let second = TEST_AGENT_MODEL_DEFAULTS
        .scope(
            AgentModelSelections {
                mode: openbitfun_core_types::model_pool::ModelSelection::Model {
                    model_ref: "model-b".into(),
                },
                ..Default::default()
            },
            coordinator.create_session_with_workspace(
                None,
                "Second".to_string(),
                "Standard".to_string(),
                SessionConfig {
                    tool_presentation_mode: Default::default(),
                    workspace_path: Some(workspace_path_string.clone()),
                    ..Default::default()
                },
                workspace_path_string.clone(),
            ),
        )
        .await
        .expect("second normal session should be created");

    assert_eq!(
        session_manager
            .get_session(&first.session_id)
            .and_then(|session| session.config.model_id.clone())
            .as_deref(),
        Some("model-a")
    );
    assert_eq!(
        session_manager
            .get_session(&second.session_id)
            .and_then(|session| session.config.model_id.clone())
            .as_deref(),
        Some("model-b")
    );

    let explicit = TEST_AGENT_MODEL_DEFAULTS
        .scope(
            AgentModelSelections {
                mode: openbitfun_core_types::model_pool::ModelSelection::Model {
                    model_ref: "model-c".into(),
                },
                ..Default::default()
            },
            coordinator.create_session_with_workspace(
                None,
                "Explicit".to_string(),
                "Standard".to_string(),
                SessionConfig {
                    tool_presentation_mode: Default::default(),
                    workspace_path: Some(workspace_path_string.clone()),
                    model_id: Some("explicit-model".to_string()),
                    ..Default::default()
                },
                workspace_path_string,
            ),
        )
        .await
        .expect("explicit-model normal session should be created");
    assert_eq!(explicit.config.model_id.as_deref(), Some("explicit-model"));

    let _ = std::fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn subagent_model_policy_is_unchanged_by_smart_routing_or_legacy_role_settings() {
    let (coordinator, manager) = test_coordinator();
    let workspace = tempfile::tempdir().unwrap();
    let record =
        crate::service::workspace::legacy_compat::register_local_fixture(workspace.path(), None)
            .await;
    let parent = manager
        .create_session(
            "Parent".into(),
            "Standard".into(),
            SessionConfig {
                model_id: Some("primary".into()),
                model_routing: Some("smart".into()),
                workspace_id: Some(record.id),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    manager
        .replace_context_messages(
            &parent.session_id,
            vec![Message::user("Inspect the workspace".into())],
        )
        .await;
    let request = |parent_id: &str, fork: bool| SubagentExecutionRequest {
        task_description: "Inspect files".into(),
        requested_agent_id: None,
        context_mode: if fork {
            SubagentContextMode::Fork
        } else {
            SubagentContextMode::Fresh
        },
        target_session_id: None,
        subagent_type: if fork { None } else { Some("Explore".into()) },
        logical_subagent_type: None,
        continuation_policy: SessionContinuationPolicy::Reusable,
        model_binding_policy: SessionModelBindingPolicy::Mutable,
        workspace_path: None,
        model_id: None,
        inherit_parent_model: false,
        subagent_parent_info: SubagentParentInfo {
            session_id: parent_id.into(),
            dialog_turn_id: "turn".into(),
            tool_call_id: "task".into(),
        },
        context: HashMap::new(),
        permission_runtime_ceiling: PermissionRuntimeCeiling::default(),
        delegation_policy: DelegationPolicy::top_level().spawn_child(),
        external_generation_lease: None,
    };
    let mut manual_config = parent.config.clone();
    manual_config.model_routing = Some("manual".into());
    let manual = manager
        .create_session("Manual".into(), "Standard".into(), manual_config)
        .await
        .unwrap();
    manager
        .replace_context_messages(
            &manual.session_id,
            vec![Message::user("Inspect the workspace".into())],
        )
        .await;
    let mut ai = AIConfig::default();
    // Previously saved smart-role values must not override existing defaults,
    // require an economy model, or change fresh/fork/nested delegation.
    ai.model_routing.subagent_model = "economy".into();
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(ai, async {
            for parent in [&parent, &manual] {
                TEST_AGENT_MODEL_DEFAULTS
                    .scope(AgentModelSelections::default(), async {
                        let fresh = coordinator
                            .resolve_hidden_subagent_execution_request(request(
                                &parent.session_id,
                                false,
                            ))
                            .await
                            .unwrap();
                        assert_eq!(fresh.session_config.model_id.as_deref(), Some("fast"));
                        let forked = coordinator
                            .resolve_hidden_subagent_execution_request(request(
                                &parent.session_id,
                                true,
                            ))
                            .await
                            .unwrap();
                        assert_eq!(forked.session_config.model_id.as_deref(), Some("primary"));
                        let mut nested_config = fresh.session_config;
                        nested_config.delegation_model_routing = Some("smart".into());
                        let nested_parent = manager
                            .create_transient_session_with_id_and_details(
                                None,
                                "Nested parent".into(),
                                "Explore".into(),
                                nested_config,
                                Some(format!("session-{}", parent.session_id)),
                                SessionKind::Subagent,
                            )
                            .await
                            .unwrap();
                        let nested = coordinator
                            .resolve_hidden_subagent_execution_request(request(
                                &nested_parent.session_id,
                                false,
                            ))
                            .await
                            .unwrap();
                        assert_eq!(nested.session_config.model_id.as_deref(), Some("fast"));
                    })
                    .await;
                let mut defaults = AgentModelSelections::default();
                defaults.subagents.builtin.insert(
                    "Explore".into(),
                    AgentModelSelection::Model {
                        model_ref: "primary".into(),
                    },
                );
                defaults.subagents.fork = AgentModelSelection::Model {
                    model_ref: "fast".into(),
                };
                TEST_AGENT_MODEL_DEFAULTS
                    .scope(defaults, async {
                        let configured = coordinator
                            .resolve_fresh_subagent_model_target(
                                None,
                                false,
                                "Explore",
                                &parent.session_id,
                            )
                            .await
                            .unwrap();
                        assert_eq!(
                            configured,
                            ModelSelection::Model {
                                model_ref: "primary".into()
                            }
                        );
                        let explicit = coordinator
                            .resolve_fresh_subagent_model_target(
                                Some("fast"),
                                false,
                                "Explore",
                                &parent.session_id,
                            )
                            .await
                            .unwrap();
                        assert_eq!(
                            explicit,
                            ModelSelection::Model {
                                model_ref: "fast".into()
                            }
                        );
                        let inherited = coordinator
                            .resolve_fresh_subagent_model_target(
                                None,
                                true,
                                "Explore",
                                &parent.session_id,
                            )
                            .await
                            .unwrap();
                        assert_eq!(
                            inherited,
                            ModelSelection::Model {
                                model_ref: "primary".into()
                            }
                        );
                        let forked = coordinator
                            .resolve_hidden_subagent_execution_request(request(
                                &parent.session_id,
                                true,
                            ))
                            .await
                            .unwrap();
                        assert_eq!(forked.session_config.model_id.as_deref(), Some("fast"));
                    })
                    .await;
            }
        })
        .await;
}

#[tokio::test]
async fn fresh_subagent_request_can_explicitly_inherit_parent_model() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-fresh-subagent-inherit-test-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&workspace_path).expect("workspace dir should exist");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace_path);
    struct TempWorkspaceGuard(std::path::PathBuf);
    impl Drop for TempWorkspaceGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _workspace_guard = TempWorkspaceGuard(workspace_path.clone());
    let parent_session = session_manager
        .create_session(
            "Parent".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                model_id: Some("primary".to_string()),
                workspace_path: Some(workspace_path.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("parent session should be created");

    let model_id = coordinator
        .resolve_fresh_subagent_model_target(None, true, "Explore", &parent_session.session_id)
        .await
        .expect("fresh subagent request should inherit the parent model");

    assert_eq!(
        model_id,
        openbitfun_core_types::model_pool::ModelSelection::Model {
            model_ref: "primary".into()
        }
    );
}

#[test]
fn approved_external_inherit_resolves_parent_once_to_a_concrete_runtime_fingerprint() {
    let model = AIModelConfig {
        id: "model-primary".to_string(),
        name: "Provider".to_string(),
        provider: "provider".to_string(),
        model_name: "model-name".to_string(),
        enabled: true,
        ..AIModelConfig::default()
    };
    let mut config = test_ai_config(vec![model.clone()]);
    config.default_models.primary = Some(model.id.clone());

    let resolved = super::super::resolve_approved_immutable_model_binding(
        &ExternalSubagentModelBinding::InheritParent,
        Some("primary"),
        &config,
    )
    .expect("inherit should materialize the current parent selection");
    assert_eq!(resolved.0, "model-primary");
    assert_eq!(resolved.1, model_runtime_binding_fingerprint(&model));

    config.models[0].enabled = false;
    assert!(super::super::resolve_approved_immutable_model_binding(
        &ExternalSubagentModelBinding::InheritParent,
        Some("primary"),
        &config,
    )
    .is_err());
}

#[test]
fn approved_external_fixed_binding_rejects_changed_runtime_configuration() {
    let mut model = AIModelConfig {
        id: "model-review".to_string(),
        name: "Provider".to_string(),
        provider: "provider".to_string(),
        model_name: "model-name".to_string(),
        enabled: true,
        ..AIModelConfig::default()
    };
    let fingerprint = model_runtime_binding_fingerprint(&model);
    let binding = ExternalSubagentModelBinding::Fixed {
        model_id: model.id.clone(),
        configuration_fingerprint: fingerprint.clone(),
    };
    let mut config = test_ai_config(vec![model.clone()]);

    assert_eq!(
        super::super::resolve_approved_immutable_model_binding(&binding, None, &config).unwrap(),
        ("model-review".to_string(), fingerprint)
    );

    model.base_url = "https://changed.example/v1".to_string();
    config.models[0] =
        openbitfun_config_contracts::split_flattened_model(&model, &mut config.providers);
    assert!(
        super::super::resolve_approved_immutable_model_binding(&binding, None, &config).is_err()
    );
}

#[test]
fn subagent_model_resolution_prioritizes_explicit_fixed_and_inherited_values() {
    assert_eq!(
        resolve_subagent_model_selection(
            Some("explicit-model"),
            &SubagentModelSelection::fixed("configured-model"),
            Some("parent-model"),
        )
        .expect("explicit model should win"),
        "explicit-model"
    );
    assert_eq!(
        resolve_subagent_model_selection(
            None,
            &SubagentModelSelection::fixed("configured-model"),
            Some("parent-model"),
        )
        .expect("configured model should win"),
        "configured-model"
    );
    assert_eq!(
        resolve_subagent_model_selection(None, &SubagentModelSelection::Inherit, Some("primary"),)
            .expect("inherit should preserve the parent selector"),
        "primary"
    );
    assert!(
        resolve_subagent_model_selection(None, &SubagentModelSelection::Inherit, None,).is_err()
    );
}
