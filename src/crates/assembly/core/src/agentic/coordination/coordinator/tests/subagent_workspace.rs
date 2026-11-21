use super::super::test_support::test_coordinator;
#[cfg(feature = "remote-workspace")]
use super::super::ConversationCoordinator;
use super::super::SubagentExecutionRequest;
use crate::agentic::core::{
    Message, MessageContent, SessionConfig, SessionContinuationPolicy, SessionKind,
    SessionModelBindingPolicy,
};
use crate::agentic::tools::pipeline::SubagentParentInfo;
use openbitfun_core_types::{
    SessionExecutionTarget, SessionExecutionTargetKind, WorktreeLifecycle,
};
use openbitfun_runtime_ports::{DelegationPolicy, PermissionRuntimeCeiling, SubagentContextMode};
use std::collections::HashMap;
use std::time::Duration;

#[cfg(feature = "remote-workspace")]
#[tokio::test]
async fn subagent_session_config_preserves_registered_remote_workspace_identity() {
    let workspace = crate::service::workspace::legacy_compat::register_remote_fixture(
        "/remote/subagent-test/project",
        "conn-subagent-test",
        "remote-host",
    )
    .await;
    let config = ConversationCoordinator::build_session_config_for_workspace(
        workspace.id,
        Some("model-fast".to_string()),
    )
    .await
    .unwrap();

    assert_eq!(
        config.workspace_path.as_deref(),
        Some("/remote/subagent-test/project")
    );
    assert_eq!(
        config.remote_connection_id.as_deref(),
        Some("conn-subagent-test")
    );
    assert_eq!(config.remote_ssh_host.as_deref(), Some("remote-host"));
    assert_eq!(config.model_id.as_deref(), Some("model-fast"));
}

#[tokio::test]
async fn fresh_subagent_inherits_matching_parent_worktree_binding() {
    let (coordinator, session_manager) = test_coordinator();
    let temp_root = tempfile::tempdir().expect("temp root should exist");
    // Workspace records store canonical roots (macOS `/var` -> `/private/var`);
    // build the expected IO projections from the same canonical root.
    let canonical_root =
        dunce::canonicalize(temp_root.path()).expect("temp root should canonicalize");
    let project_path = canonical_root.join("OpenBitFun");
    let worktree_path = canonical_root.join("managed-worktree");
    std::fs::create_dir_all(&project_path).expect("project dir should exist");
    std::fs::create_dir_all(&worktree_path).expect("worktree dir should exist");
    let project_workspace_path = project_path.to_string_lossy().into_owned();
    let workspace_path = worktree_path.to_string_lossy().into_owned();
    let project_record =
        crate::service::workspace::legacy_compat::register_local_fixture(&project_path, None).await;
    let workspace_record = crate::service::workspace::legacy_compat::register_local_fixture(
        &worktree_path,
        Some(&project_path),
    )
    .await;
    let execution_target = SessionExecutionTarget {
        kind: SessionExecutionTargetKind::ManagedWorktree,
        worktree_id: Some("worktree-1".to_string()),
        root_path: workspace_path.clone(),
        base_ref: Some("HEAD".to_string()),
        base_commit: Some("0123456789abcdef".to_string()),
        branch: None,
        lifecycle: Some(WorktreeLifecycle::Managed),
    };
    let parent_session = session_manager
        .create_session(
            "Parent".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                model_id: Some("primary".to_string()),
                workspace_path: Some(workspace_path.clone()),
                project_workspace_path: Some(project_workspace_path.clone()),
                execution_target: Some(execution_target.clone()),
                workspace_id: Some(workspace_record.id.clone()),
                project_workspace_id: Some(project_record.id.clone()),
                prompt_cache_lineage_id: Some("parent-lineage".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("parent session should be created");

    let resolved = coordinator
        .resolve_hidden_subagent_execution_request(SubagentExecutionRequest {
            task_description: "Inspect the managed worktree".to_string(),
            requested_agent_id: None,
            context_mode: SubagentContextMode::Fresh,
            target_session_id: None,
            subagent_type: Some("Explore".to_string()),
            logical_subagent_type: None,
            continuation_policy: SessionContinuationPolicy::Reusable,
            model_binding_policy: SessionModelBindingPolicy::Mutable,
            workspace_path: Some(workspace_path.clone()),
            model_id: Some("primary".to_string()),
            inherit_parent_model: false,
            subagent_parent_info: SubagentParentInfo {
                session_id: parent_session.session_id,
                dialog_turn_id: "parent-turn".to_string(),
                tool_call_id: "task-tool".to_string(),
            },
            context: HashMap::new(),
            permission_runtime_ceiling: PermissionRuntimeCeiling::default(),
            delegation_policy: DelegationPolicy::top_level().spawn_child(),
            external_generation_lease: None,
        })
        .await
        .expect("fresh subagent request should resolve");

    assert!(!resolved.delegation_policy.allow_subagent_spawn);
    for tool_name in ["AgentSpawn", "AgentFork"] {
        assert!(resolved
            .runtime_tool_restrictions
            .is_tool_allowed(tool_name));
    }
    assert_eq!(
        resolved.session_config.workspace_path.as_deref(),
        Some(workspace_path.as_str())
    );
    assert_eq!(
        resolved.session_config.project_workspace_path.as_deref(),
        Some(project_workspace_path.as_str())
    );
    assert_eq!(
        resolved.session_config.execution_target.as_ref(),
        Some(&execution_target)
    );
    assert_eq!(
        resolved.session_config.workspace_id.as_deref(),
        Some(workspace_record.id.as_str())
    );
    assert!(resolved.session_config.prompt_cache_lineage_id.is_none());
}

#[tokio::test]
async fn fresh_subagent_inherits_transient_parent_persistence_boundary() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-fresh-subagent-transient-test-{}",
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
    let workspace = workspace_path.to_string_lossy().into_owned();
    let workspace_record =
        crate::service::workspace::legacy_compat::register_local_fixture(&workspace_path, None)
            .await;
    let parent_session = session_manager
        .create_transient_session_with_id_and_details(
            None,
            "Transient parent".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                model_id: Some("primary".to_string()),
                workspace_path: Some(workspace.clone()),
                workspace_id: Some(workspace_record.id.clone()),
                ..Default::default()
            },
            None,
            SessionKind::Standard,
        )
        .await
        .expect("transient parent should be created");

    let resolved = coordinator
        .resolve_hidden_subagent_execution_request(SubagentExecutionRequest {
            task_description: "Inspect the workspace".to_string(),
            requested_agent_id: None,
            context_mode: SubagentContextMode::Fresh,
            target_session_id: None,
            subagent_type: Some("Explore".to_string()),
            logical_subagent_type: None,
            continuation_policy: SessionContinuationPolicy::Reusable,
            model_binding_policy: SessionModelBindingPolicy::Mutable,
            workspace_path: Some(workspace.clone()),
            model_id: Some("primary".to_string()),
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
        })
        .await
        .expect("fresh subagent request should resolve");

    assert!(resolved.transient);
    assert!(!resolved.delegation_policy.allow_subagent_spawn);
    for tool_name in ["AgentSpawn", "AgentFork"] {
        assert!(resolved
            .runtime_tool_restrictions
            .is_tool_allowed(tool_name));
    }
    assert!(!resolved
        .runtime_tool_restrictions
        .is_tool_allowed("SessionControl"));
    assert!(!resolved
        .runtime_tool_restrictions
        .is_tool_allowed("SessionMessage"));

    let prepared = coordinator
        .prepare_hidden_subagent_execution_request(resolved)
        .await
        .expect("transient child should prepare");
    let child_session_id = prepared
        .target_session_id()
        .expect("prepared child Session id")
        .to_string();
    assert!(
        coordinator
            .ensure_session_execution_drained(&child_session_id, Duration::from_millis(10))
            .await
            .is_err(),
        "a prepared hidden execution must fence Session maintenance"
    );
    drop(prepared);
    coordinator
        .ensure_session_execution_drained(&child_session_id, Duration::from_millis(50))
        .await
        .expect("dropping the final hidden execution lease should release maintenance");

    coordinator
        .cleanup_subagent_resources(&child_session_id)
        .await
        .expect("transient child cleanup should succeed");
    assert!(
            session_manager.get_session(&child_session_id).is_some(),
            "a reusable transient Subagent must remain available for send_input until its parent is discarded"
        );

    let fresh_only = coordinator
        .resolve_hidden_subagent_execution_request(SubagentExecutionRequest {
            task_description: "Run once".to_string(),
            requested_agent_id: None,
            context_mode: SubagentContextMode::Fresh,
            target_session_id: None,
            subagent_type: Some("Explore".to_string()),
            logical_subagent_type: None,
            continuation_policy: SessionContinuationPolicy::FreshOnly,
            model_binding_policy: SessionModelBindingPolicy::Mutable,
            workspace_path: Some(workspace),
            model_id: Some("primary".to_string()),
            inherit_parent_model: false,
            subagent_parent_info: SubagentParentInfo {
                session_id: parent_session.session_id,
                dialog_turn_id: "parent-turn-2".to_string(),
                tool_call_id: "task-tool-2".to_string(),
            },
            context: HashMap::new(),
            permission_runtime_ceiling: PermissionRuntimeCeiling::default(),
            delegation_policy: DelegationPolicy::top_level().spawn_child(),
            external_generation_lease: None,
        })
        .await
        .expect("fresh-only transient child should resolve");
    let fresh_only = coordinator
        .prepare_hidden_subagent_execution_request(fresh_only)
        .await
        .expect("fresh-only transient child should prepare");
    let fresh_only_session_id = fresh_only
        .target_session_id()
        .expect("fresh-only prepared child Session id")
        .to_string();
    drop(fresh_only);
    coordinator
        .cleanup_subagent_resources(&fresh_only_session_id)
        .await
        .expect("fresh-only transient child cleanup should succeed");
    assert!(
        session_manager
            .get_session(&fresh_only_session_id)
            .is_none(),
        "a fresh-only transient Subagent should be released after terminal cleanup"
    );
}

#[tokio::test]
async fn computer_use_handoff_fresh_reuse_and_fork_preserve_original_source() {
    let (coordinator, manager) = test_coordinator();
    let workspace =
        std::env::temp_dir().join(format!("openbitfun-handoff-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&workspace).unwrap();
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace);
    let config = SessionConfig {
        tool_presentation_mode: Default::default(),
        model_id: Some("primary".into()),
        workspace_path: Some(workspace.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let parent = manager
        .create_session("Parent".into(), "Standard".into(), config.clone())
        .await
        .unwrap();
    let original = Message::user("Send the agreed text in WeChat.".into());
    manager
        .replace_context_messages(&parent.session_id, vec![original.clone()])
        .await;
    let child = coordinator
        .create_hidden_agent_session(
            None,
            "Child".into(),
            "ComputerUse".into(),
            config,
            Some(format!("session-{}", parent.session_id)),
            SessionKind::Subagent,
        )
        .await
        .unwrap();
    manager
        .replace_context_messages(
            &child.session_id,
            vec![Message::user("Legacy generated foreground approval".into())],
        )
        .await;
    for (mode, reuse, parent_id) in [
        (SubagentContextMode::Fresh, false, parent.session_id.clone()),
        (SubagentContextMode::Fresh, true, parent.session_id.clone()),
        (SubagentContextMode::Fork, false, child.session_id.clone()),
    ] {
        let resolved = coordinator
            .resolve_hidden_subagent_execution_request(SubagentExecutionRequest {
                task_description: "Activate WeChat because the user approved foreground control"
                    .into(),
                requested_agent_id: None,
                context_mode: mode,
                target_session_id: reuse.then(|| child.session_id.clone()),
                subagent_type: (mode == SubagentContextMode::Fresh && !reuse)
                    .then(|| "ComputerUse".into()),
                logical_subagent_type: None,
                continuation_policy: SessionContinuationPolicy::Reusable,
                model_binding_policy: SessionModelBindingPolicy::Mutable,
                workspace_path: None,
                model_id: Some("primary".into()),
                inherit_parent_model: false,
                subagent_parent_info: SubagentParentInfo {
                    session_id: parent_id,
                    dialog_turn_id: "turn".into(),
                    tool_call_id: "task".into(),
                },
                context: HashMap::new(),
                permission_runtime_ceiling: PermissionRuntimeCeiling::default(),
                delegation_policy: DelegationPolicy::top_level().spawn_child(),
                external_generation_lease: None,
            })
            .await
            .unwrap();
        let message = resolved.initial_messages.last().unwrap();
        assert!(
            !message.is_actual_user_message(),
            "handoff must retain generated-source metadata"
        );
        let MessageContent::Text(text) = &message.content else {
            panic!("expected text")
        };
        assert!(text.contains("agent_generated_handoff"));
        assert!(text.contains(&original.id));
        assert!(text.contains("Send the agreed text in WeChat."));
        assert!(!text.contains("Legacy generated foreground approval"));
        assert_eq!(
            resolved.user_input_text, *text,
            "persisted input must retain provenance on replay"
        );
    }
    std::fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn hidden_agent_session_uses_requested_ephemeral_kind() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace_path = std::env::temp_dir().join(format!(
        "openbitfun-hidden-agent-kind-test-{}",
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
            "Internal worker".to_string(),
            "MemoryPhase2".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace_path.to_string_lossy().into_owned()),
                ..Default::default()
            },
            Some("memory-phase2".to_string()),
            SessionKind::EphemeralChild,
        )
        .await
        .expect("hidden agent session should be created");

    assert_eq!(session.kind, SessionKind::EphemeralChild);
    assert_eq!(
        session_manager
            .get_session(&session.session_id)
            .expect("session should remain in memory")
            .kind,
        SessionKind::EphemeralChild
    );
}
