use super::super::test_support::{test_ai_config, test_coordinator, test_persistent_coordinator};
use super::super::SubagentExecutionRequest;
use crate::agentic::core::{
    Message, MessageContent, MessageRole, MessageSemanticKind, SessionConfig,
    SessionContinuationPolicy, SessionKind, SessionModelBindingPolicy,
};
use crate::agentic::session::TEST_MODEL_RESOLUTION_AI_CONFIG;
use crate::agentic::tools::pipeline::SubagentParentInfo;
use crate::service::config::types::{AIConfig, AIModelConfig};
use openbitfun_agent_runtime::permission::AUTO_APPROVE_ASK_CONTEXT_KEY;
use openbitfun_runtime_ports::{
    DelegationPolicy, PermissionEffect, PermissionRule, PermissionRuntimeCeiling,
    SubagentContextMode,
};
use std::collections::HashMap;

#[tokio::test]
async fn current_context_estimate_supports_fresh_reused_and_forked_subagents() {
    use crate::service::session::{DialogTurnData, UserMessageData};
    use openbitfun_core_types::{UsageContextCategory, UsageContextEstimateRequest};

    let workspace = tempfile::tempdir().unwrap();
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    std::fs::write(
        workspace.path().join("AGENTS.md"),
        "Child workspace instruction",
    )
    .unwrap();
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(context_estimate_ai_config(), async {
            let (coordinator, manager) = test_persistent_coordinator();
            let persistence = manager.persistence_manager();
            let parent = manager
                .create_session(
                    "Parent".into(),
                    "Standard".into(),
                    SessionConfig {
                        workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                        model_id: Some("parent-model".into()),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            let parent_message =
                Message::user("Parent context is inherited only by forks. ".repeat(100));
            manager
                .replace_context_messages(&parent.session_id, vec![parent_message.clone()])
                .await;

            let mut fresh_child = None;
            let mut fresh_conversation = 0;
            for (index, context_mode) in [
                SubagentContextMode::Fresh,
                SubagentContextMode::Fresh,
                SubagentContextMode::Fork,
            ]
            .into_iter()
            .enumerate()
            {
                let target = (index == 1).then(|| fresh_child.clone().unwrap());
                let prepared = coordinator
                    .prepare_subagent_execution_request(SubagentExecutionRequest {
                        task_description: format!("Inspect child context {index}"),
                        requested_agent_id: None,
                        context_mode,
                        target_session_id: target.clone(),
                        subagent_type: (target.is_none()
                            && context_mode == SubagentContextMode::Fresh)
                            .then(|| "GeneralPurpose".into()),
                        logical_subagent_type: None,
                        continuation_policy: SessionContinuationPolicy::Reusable,
                        model_binding_policy: SessionModelBindingPolicy::Mutable,
                        workspace_path: None,
                        model_id: Some("child-model".into()),
                        inherit_parent_model: false,
                        subagent_parent_info: SubagentParentInfo {
                            session_id: parent.session_id.clone(),
                            dialog_turn_id: "parent-turn".into(),
                            tool_call_id: format!("spawn-{index}"),
                        },
                        context: HashMap::new(),
                        permission_runtime_ceiling: PermissionRuntimeCeiling::default(),
                        delegation_policy: DelegationPolicy::top_level().spawn_child(),
                        external_generation_lease: None,
                    })
                    .await
                    .unwrap();
                let child_id = prepared.target_session_id().unwrap().to_string();
                if index == 0 {
                    fresh_child = Some(child_id.clone());
                }
                let messages = prepared.initial_messages.clone();
                drop(prepared);
                manager
                    .replace_context_messages(&child_id, messages.clone())
                    .await;
                let storage = manager
                    .effective_session_storage_path(&child_id)
                    .await
                    .unwrap();
                let turn = DialogTurnData::new(
                    format!("child-turn-{index}"),
                    index,
                    child_id.clone(),
                    UserMessageData {
                        id: format!("child-input-{index}"),
                        content: format!("Inspect child context {index}"),
                        timestamp: 1,
                        metadata: None,
                    },
                );
                persistence.save_dialog_turn(&storage, &turn).await.unwrap();
                persistence
                    .save_turn_context_snapshot(&storage, &child_id, index, &messages)
                    .await
                    .unwrap();
                let root = if persistence.is_resolved_sessions_dir(&storage) {
                    storage.clone()
                } else {
                    persistence.path_manager().project_sessions_dir(&storage)
                };
                let child_dir = root.join(&child_id);
                let before = context_estimate_session_files(&child_dir);
                let session_before =
                    serde_json::to_value(manager.get_session(&child_id).unwrap()).unwrap();
                let mut estimates = Vec::new();
                for include_contents in [false, true] {
                    let inputs = manager
                        .read_context_estimate_inputs(&storage, &child_id)
                        .await
                        .unwrap();
                    assert_eq!(inputs.session.kind, SessionKind::Subagent);
                    let estimate = coordinator
                        .estimate_session_context(
                            inputs,
                            &UsageContextEstimateRequest {
                                include_contents,
                                context_only: true,
                            },
                        )
                        .await
                        .unwrap();
                    assert!(estimate.unavailable_reason.is_none());
                    assert_eq!(estimate.contents.is_empty(), !include_contents);
                    estimates.push(estimate);
                }
                assert_eq!(estimates[0].composition, estimates[1].composition);
                let composition = estimates[0].composition.as_ref().unwrap();
                assert_eq!(composition.context_window, Some(256_000));
                assert!(composition.system_prompt > 0);
                assert!(composition.tool_definitions > 0);
                assert!(composition.rules > 0);
                assert!(composition.conversation > 0);
                let preview = &estimates[1].contents;
                assert!(
                    preview[&UsageContextCategory::Rules].contains("Child workspace instruction")
                );
                assert!(
                    preview[&UsageContextCategory::ToolDefinitions].contains("\"name\": \"Read\"")
                );
                for name in ["AgentSpawn", "AgentFork"] {
                    assert_eq!(
                        preview[&UsageContextCategory::ToolDefinitions]
                            .contains(&format!("\"name\": \"{name}\"")),
                        context_mode == SubagentContextMode::Fork,
                        "estimates must retain each agent's own tool policy"
                    );
                }
                if index == 0 {
                    fresh_conversation = composition.conversation;
                    assert!(!messages
                        .iter()
                        .any(|message| message.id == parent_message.id));
                } else {
                    assert!(composition.conversation > fresh_conversation);
                }
                assert_eq!(before, context_estimate_session_files(&child_dir));
                assert_eq!(
                    session_before,
                    serde_json::to_value(manager.get_session(&child_id).unwrap()).unwrap()
                );

                // Persisted children must remain inspectable without being restored
                // or depending on a legacy display snapshot.
                manager.unload_session_from_memory(&child_id).await.unwrap();
                std::fs::write(
                    child_dir.join("usage-context.json"),
                    b"unreadable legacy display snapshot",
                )
                .unwrap();
                let before = context_estimate_session_files(&child_dir);
                let inputs = manager
                    .read_context_estimate_inputs(&storage, &child_id)
                    .await
                    .unwrap();
                let restored = coordinator
                    .estimate_session_context(inputs, &UsageContextEstimateRequest::default())
                    .await
                    .unwrap();
                assert_eq!(restored.composition, estimates[0].composition);
                assert!(manager.get_session(&child_id).is_none());
                assert_eq!(before, context_estimate_session_files(&child_dir));
            }
        })
        .await;
}

#[tokio::test]
async fn current_context_estimate_remote_subagent_uses_cached_rules_without_local_fallback() {
    use crate::agentic::agents::get_agent_registry;
    use crate::agentic::session::CachedUserContext;
    use crate::agentic::tools::ToolRuntimeRestrictions;
    use crate::agentic::WorkspaceBinding;
    use crate::service::remote_ssh::workspace_state::WorkspaceSessionIdentity;
    use openbitfun_core_types::{UsageContextCategory, UsageContextEstimateRequest, WorkspaceKind};

    let local = tempfile::tempdir().unwrap();
    std::fs::write(
        local.path().join("AGENTS.md"),
        "Controller-local private rules",
    )
    .unwrap();
    // A same-named controller path must never supply remote workspace rules.
    let workspace = WorkspaceBinding::new_remote(
        None,
        local.path().to_path_buf(),
        "context-estimate-connection".into(),
        "remote-host".into(),
        WorkspaceSessionIdentity {
            workspace_kind: WorkspaceKind::Remote,
            hostname: "remote-host".into(),
            logical_workspace_path: local.path().to_string_lossy().into_owned(),
            remote_connection_id: Some("context-estimate-connection".into()),
        },
    );
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(context_estimate_ai_config(), async {
            let (coordinator, _) = test_coordinator();
            let agent = get_agent_registry()
                .get_agent("GeneralPurpose", None)
                .unwrap();
            for (kind, cached) in [
                (SessionKind::Subagent, false),
                (SessionKind::Subagent, true),
                (SessionKind::EphemeralChild, false),
                (SessionKind::EphemeralChild, true),
            ] {
                let mut inputs = context_estimate_subagent_inputs(&workspace);
                inputs.session.kind = kind;
                if kind == SessionKind::EphemeralChild {
                    inputs.session.created_by = Some("session-parent".into());
                }
                if cached {
                    let identity = agent.user_context_cache_identity();
                    #[cfg(feature = "remote-workspace")]
                    let identity = crate::agentic::session::UserContextCacheIdentity::new(format!(
                        "{}|remote:remote-host",
                        identity.scope_key
                    ));
                    inputs.prompt_cache.user_context = Some(CachedUserContext::new(
                        identity,
                        "Remote child workspace rules",
                    ));
                }
                let result = coordinator
                    .execution_engine
                    .estimate_current_context(
                        inputs,
                        workspace.clone(),
                        None,
                        coordinator.terminal_port.get(),
                        coordinator.remote_exec_port.get(),
                        ToolRuntimeRestrictions::default(),
                        HashMap::new(),
                        &UsageContextEstimateRequest {
                            include_contents: true,
                            context_only: true,
                        },
                    )
                    .await;
                if cached {
                    let estimate = result.unwrap();
                    assert!(estimate.composition.unwrap().rules > 0);
                    let rules = &estimate.contents[&UsageContextCategory::Rules];
                    assert!(rules.contains("Remote child workspace rules"));
                    assert!(!rules.contains("Controller-local private rules"));
                } else {
                    assert!(result
                        .unwrap_err()
                        .to_string()
                        .contains("Workspace instructions are unavailable"));
                }
            }
        })
        .await;
}

#[tokio::test]
async fn current_context_estimate_uses_the_model_window_with_legacy_session_limits() {
    use crate::agentic::core::Session;
    use openbitfun_core_types::UsageContextEstimateRequest;

    let root = tempfile::tempdir().unwrap();
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(root.path());
    let mut config = context_estimate_ai_config();
    config.models[1].context_window = Some(1_048_576);
    config.models[1].max_tokens = Some(384_000);
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(config, async {
            let (coordinator, manager) = test_persistent_coordinator();
            for (saved_window, model, expected_window) in [
                (128_128, "child-model", 1_048_576),
                (1_048_576, "parent-model", 64_000),
            ] {
                let session = Session::new(
                    "Saved session".into(),
                    "Standard".into(),
                    SessionConfig {
                        workspace_path: Some(root.path().to_string_lossy().into_owned()),
                        model_id: Some(model.into()),
                        max_context_tokens: saved_window,
                        ..Default::default()
                    },
                );
                manager
                    .persistence_manager()
                    .save_session(root.path(), &session)
                    .await
                    .unwrap();
                let inputs = manager
                    .read_context_estimate_inputs(root.path(), &session.session_id)
                    .await
                    .unwrap();
                assert_eq!(inputs.session.config.max_context_tokens, saved_window);
                let estimate = coordinator
                    .estimate_session_context(inputs, &UsageContextEstimateRequest::default())
                    .await
                    .unwrap();
                assert_eq!(
                    estimate.composition.unwrap().context_window,
                    Some(expected_window)
                );
            }
        })
        .await;
}

#[tokio::test]
async fn current_context_estimate_keeps_external_and_internal_worker_contexts_unsupported() {
    use crate::agentic::tools::ToolRuntimeRestrictions;
    use crate::agentic::WorkspaceBinding;
    use openbitfun_core_types::{SessionAgentRouteOwner, UsageContextEstimateRequest};

    let (coordinator, _) = test_coordinator();
    let workspace = WorkspaceBinding::new(None, std::env::temp_dir());
    for (kind, route, agent_type, reason) in [
        (
            SessionKind::EphemeralChild,
            SessionAgentRouteOwner::Local,
            "GeneralPurpose",
            "execution-scoped tool restrictions",
        ),
        (
            SessionKind::Subagent,
            SessionAgentRouteOwner::External,
            "GeneralPurpose",
            "external agent",
        ),
        (
            SessionKind::Subagent,
            SessionAgentRouteOwner::Local,
            "acp:codex",
            "external agent",
        ),
    ] {
        let mut inputs = context_estimate_subagent_inputs(&workspace);
        inputs.session.kind = kind;
        inputs.session.config.agent_route_owner = route;
        inputs.session.agent_type = agent_type.into();
        let error = coordinator
            .execution_engine
            .estimate_current_context(
                inputs,
                workspace.clone(),
                None,
                None,
                None,
                ToolRuntimeRestrictions::default(),
                HashMap::new(),
                &UsageContextEstimateRequest::default(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains(reason));
    }
}

fn context_estimate_subagent_inputs(
    workspace: &crate::agentic::WorkspaceBinding,
) -> crate::agentic::session::session_manager::ContextEstimateInputs {
    use crate::agentic::core::Session;
    use crate::agentic::session::session_manager::ContextEstimateInputs;

    let mut session = Session::new(
        "Child".into(),
        "GeneralPurpose".into(),
        SessionConfig {
            workspace_path: Some(workspace.root_path_string()),
            model_id: Some("child-model".into()),
            ..Default::default()
        },
    );
    session.kind = SessionKind::Subagent;
    ContextEstimateInputs {
        session,
        messages: vec![Message::user("Child task".into())],
        prompt_cache: Default::default(),
        cache_ttl: None,
        listing_baseline: None,
        turn_id: "child-turn".into(),
        turn_metadata: None,
        turn_index: 0,
    }
}

fn context_estimate_ai_config() -> AIConfig {
    let mut config = test_ai_config(
        [("parent-model", 64_000), ("child-model", 256_000)]
            .into_iter()
            .map(|(id, context_window)| AIModelConfig {
                id: id.into(),
                model_name: id.into(),
                enabled: true,
                context_window: Some(context_window),
                ..Default::default()
            })
            .collect(),
    );
    config.default_models.primary = Some("parent-model".into());
    config
}

fn context_estimate_session_files(
    path: &std::path::Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(context_estimate_session_files(&path));
        } else {
            files.insert(path.clone(), std::fs::read(path).unwrap());
        }
    }
    files
}

#[tokio::test]
async fn reused_subagent_send_input_updates_requested_and_inherited_model() {
    assert_reused_subagent_model_selection(None).await;
}

#[tokio::test]
async fn reused_subagent_legacy_smart_keeps_target_until_explicit_model_replacement() {
    // Typed target replacement validates the selected configured model.
    // Supply the same host-owned configuration a real runtime has loaded.
    let mut config = test_ai_config(
        ["main-model", "worker-model", "parent-model"]
            .into_iter()
            .map(|id| AIModelConfig {
                id: id.into(),
                enabled: true,
                ..Default::default()
            })
            .collect(),
    );
    config.default_models.primary = Some("main-model".into());
    config.default_models.fast = Some("worker-model".into());
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            config,
            assert_reused_subagent_model_selection(Some("smart")),
        )
        .await;
}

async fn assert_reused_subagent_model_selection(legacy_routing: Option<&str>) {
    let (coordinator, session_manager) = test_persistent_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-reused-subagent-model-test-{}",
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

    let subagent_session = coordinator
        .create_hidden_agent_session(
            None,
            "Reusable subagent".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                model_id: Some("parent-model".to_string()),
                model_routing: legacy_routing.map(str::to_owned),
                delegation_model_routing: legacy_routing.map(str::to_owned),
                workspace_path: Some(workspace_path.to_string_lossy().into_owned()),
                ..Default::default()
            },
            Some(format!("session-{}", parent_session.session_id)),
            SessionKind::Subagent,
        )
        .await
        .expect("subagent session should be created");

    assert!(
        session_manager
            .unload_session_from_memory(&subagent_session.session_id)
            .await
            .expect("persisted subagent session should unload"),
        "the restart regression requires the target subagent to be absent from memory"
    );
    assert!(
        session_manager
            .get_session(&subagent_session.session_id)
            .is_none(),
        "the send_input preparation must restore the persisted subagent on demand"
    );

    let request = SubagentExecutionRequest {
        task_description: "Continue the investigation".to_string(),
        requested_agent_id: None,
        context_mode: SubagentContextMode::Fresh,
        target_session_id: Some(subagent_session.session_id.clone()),
        subagent_type: None,
        logical_subagent_type: None,
        continuation_policy: SessionContinuationPolicy::Reusable,
        model_binding_policy: SessionModelBindingPolicy::Mutable,
        workspace_path: None,
        model_id: Some("fast".to_string()),
        inherit_parent_model: false,
        subagent_parent_info: SubagentParentInfo {
            session_id: parent_session.session_id.clone(),
            dialog_turn_id: "parent-turn".to_string(),
            tool_call_id: "task-tool".to_string(),
        },
        context: HashMap::from([(
            AUTO_APPROVE_ASK_CONTEXT_KEY.to_string(),
            "false".to_string(),
        )]),
        permission_runtime_ceiling: PermissionRuntimeCeiling::try_new(vec![
            PermissionRule::new("bash", "rm *", PermissionEffect::Deny),
            PermissionRule::new("external_directory", "*", PermissionEffect::Ask),
        ])
        .expect("test ceiling should be valid"),
        delegation_policy: DelegationPolicy::top_level().spawn_child(),
        external_generation_lease: None,
    };

    let mut unchanged_request = request.clone();
    unchanged_request.model_id = None;
    let unchanged = coordinator
        .resolve_hidden_subagent_execution_request(unchanged_request)
        .await
        .expect("restored child should retain its saved target without a model override");
    assert_eq!(
        unchanged.session_config.model_id.as_deref(),
        Some("parent-model")
    );
    assert_eq!(
        unchanged.session_config.model_routing.as_deref(),
        legacy_routing
    );
    assert_eq!(
        unchanged.session_config.delegation_model_routing.as_deref(),
        legacy_routing
    );
    let restored = session_manager
        .get_session(&subagent_session.session_id)
        .unwrap();
    assert!(
        !crate::agentic::execution::round_model_routing::configuration::session_eligible(&restored)
    );

    let prepared = coordinator
        .prepare_subagent_execution_request(request)
        .await
        .expect("send_input request should prepare with a requested model");

    assert!(!prepared.delegation_policy.allow_subagent_spawn);
    for tool_name in ["AgentSpawn", "AgentFork"] {
        assert!(prepared
            .runtime_tool_restrictions
            .is_tool_allowed(tool_name));
    }
    assert_eq!(prepared.session_config.model_id.as_deref(), Some("fast"));
    if legacy_routing.is_some() {
        assert_eq!(
            prepared.session_config.model_routing.as_deref(),
            Some("manual")
        );
        assert_eq!(
            prepared.session_config.delegation_model_routing.as_deref(),
            legacy_routing
        );
    }
    assert_eq!(
        prepared
            .context
            .get(AUTO_APPROVE_ASK_CONTEXT_KEY)
            .map(String::as_str),
        Some("false"),
        "reused subagent runs must use the current invocation override"
    );
    assert_eq!(
        prepared
            .permission_runtime_ceiling
            .as_ref()
            .expect("child request should retain the parent ceiling")
            .rules(),
        [
            PermissionRule::new("bash", "rm *", PermissionEffect::Deny),
            PermissionRule::new("external_directory", "*", PermissionEffect::Ask,),
        ]
    );
    assert_eq!(
        session_manager
            .get_session(&subagent_session.session_id)
            .expect("subagent session should remain available")
            .config
            .model_id
            .as_deref(),
        Some("fast")
    );

    let inherit_request = SubagentExecutionRequest {
        task_description: "Continue with the parent model".to_string(),
        requested_agent_id: None,
        context_mode: SubagentContextMode::Fresh,
        target_session_id: Some(subagent_session.session_id.clone()),
        subagent_type: None,
        logical_subagent_type: None,
        continuation_policy: SessionContinuationPolicy::Reusable,
        model_binding_policy: SessionModelBindingPolicy::Mutable,
        workspace_path: None,
        model_id: None,
        inherit_parent_model: true,
        subagent_parent_info: SubagentParentInfo {
            session_id: parent_session.session_id.clone(),
            dialog_turn_id: "parent-turn".to_string(),
            tool_call_id: "task-tool".to_string(),
        },
        context: HashMap::new(),
        permission_runtime_ceiling: PermissionRuntimeCeiling::default(),
        delegation_policy: DelegationPolicy::top_level().spawn_child(),
        external_generation_lease: None,
    };

    let prepared = coordinator
        .prepare_subagent_execution_request(inherit_request)
        .await
        .expect("send_input request should inherit the parent model");

    assert_eq!(prepared.session_config.model_id.as_deref(), Some("primary"));
    assert_eq!(
        session_manager
            .get_session(&subagent_session.session_id)
            .expect("subagent session should remain available")
            .config
            .model_id
            .as_deref(),
        Some("primary")
    );
}

#[tokio::test]
async fn fork_subagent_request_allows_requested_model_override() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-fork-model-test-{}",
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
    session_manager
        .replace_context_messages(
            &parent_session.session_id,
            vec![Message::user("parent context".to_string())],
        )
        .await;

    let request = SubagentExecutionRequest {
        task_description: "Fork and inspect the repo".to_string(),
        requested_agent_id: None,
        context_mode: SubagentContextMode::Fork,
        target_session_id: None,
        subagent_type: None,
        logical_subagent_type: None,
        continuation_policy: SessionContinuationPolicy::Reusable,
        model_binding_policy: SessionModelBindingPolicy::Mutable,
        workspace_path: None,
        model_id: Some("fast".to_string()),
        inherit_parent_model: false,
        subagent_parent_info: SubagentParentInfo {
            session_id: parent_session.session_id.clone(),
            dialog_turn_id: "parent-turn".to_string(),
            tool_call_id: "task-tool".to_string(),
        },
        context: HashMap::new(),
        permission_runtime_ceiling: PermissionRuntimeCeiling::default(),
        delegation_policy: DelegationPolicy::top_level().spawn_child(),
        external_generation_lease: None,
    };

    let prepared = coordinator
        .prepare_subagent_execution_request(request)
        .await
        .expect("fork request should prepare with a requested model");

    assert_eq!(prepared.session_config.model_id.as_deref(), Some("fast"));
    assert!(!prepared.context.contains_key(AUTO_APPROVE_ASK_CONTEXT_KEY));
    assert_eq!(
        prepared.prompt_cache_source_session_id.as_deref(),
        Some(parent_session.session_id.as_str())
    );
    assert_eq!(
        session_manager
            .get_session(prepared.target_session_id().expect("prepared session id"))
            .expect("forked subagent session should exist")
            .config
            .model_id
            .as_deref(),
        Some("fast")
    );

    let inherit_request = SubagentExecutionRequest {
        task_description: "Fork with the parent model".to_string(),
        requested_agent_id: None,
        context_mode: SubagentContextMode::Fork,
        target_session_id: None,
        subagent_type: None,
        logical_subagent_type: None,
        continuation_policy: SessionContinuationPolicy::Reusable,
        model_binding_policy: SessionModelBindingPolicy::Mutable,
        workspace_path: None,
        model_id: None,
        inherit_parent_model: true,
        subagent_parent_info: SubagentParentInfo {
            session_id: parent_session.session_id.clone(),
            dialog_turn_id: "parent-turn".to_string(),
            tool_call_id: "task-tool".to_string(),
        },
        context: HashMap::new(),
        permission_runtime_ceiling: PermissionRuntimeCeiling::default(),
        delegation_policy: DelegationPolicy::top_level().spawn_child(),
        external_generation_lease: None,
    };

    let prepared = coordinator
        .prepare_subagent_execution_request(inherit_request)
        .await
        .expect("fork request should inherit the parent model");

    assert_eq!(prepared.session_config.model_id.as_deref(), Some("primary"));
    for tool_name in ["Task", "AgentSpawn", "AgentFork"] {
        assert!(prepared
            .runtime_tool_restrictions
            .is_tool_allowed(tool_name));
    }
    assert!(!prepared.delegation_policy.allow_subagent_spawn);
}

#[tokio::test]
async fn reused_subagent_input_is_added_to_runtime_context() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-reused-subagent-input-context-test-{}",
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

    let session = coordinator
        .create_hidden_agent_session(
            None,
            "Reusable subagent".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace_path.to_string_lossy().into_owned()),
                ..Default::default()
            },
            Some("session-parent".to_string()),
            SessionKind::Subagent,
        )
        .await
        .expect("subagent session should be created");
    session_manager
        .replace_context_messages(
            &session.session_id,
            vec![Message::assistant("previous answer".to_string())],
        )
        .await;

    let turn_id = session_manager
        .start_dialog_turn_with_existing_context(
            &session.session_id,
            "Standard".to_string(),
            "continue investigation".to_string(),
            Some("subagent-turn-reuse".to_string()),
            None,
        )
        .await
        .expect("turn should start");
    coordinator
        .persist_reused_subagent_user_input_context_if_needed(
            Some(&session.session_id),
            false,
            &session.session_id,
            &turn_id,
            "continue investigation",
        )
        .await
        .expect("user input context should persist");

    let context_messages = session_manager
        .get_context_messages(&session.session_id)
        .await
        .expect("context should be readable");
    assert_eq!(context_messages.len(), 2);
    let user_message = context_messages.last().expect("user message should exist");
    assert_eq!(user_message.role, MessageRole::User);
    assert_eq!(
        user_message.metadata.turn_id.as_deref(),
        Some("subagent-turn-reuse")
    );
    assert_eq!(
        user_message.metadata.semantic_kind,
        Some(MessageSemanticKind::ActualUserInput)
    );
    assert!(matches!(
        &user_message.content,
        MessageContent::Text(text) if text == "continue investigation"
    ));
}
