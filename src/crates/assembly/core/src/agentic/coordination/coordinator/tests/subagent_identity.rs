use super::super::test_support::{
    register_test_background_task, test_coordinator, test_persistent_coordinator,
};
use super::super::{
    build_subagent_session_relationship, external_delegation_agent_id,
    logical_subagent_type_or_runtime,
};
use crate::agentic::coordination::coordination_store::BackgroundTaskRegistration;
use crate::agentic::core::{Message, SessionConfig, SessionContinuationPolicy, ToolCall};
use crate::agentic::tools::pipeline::SubagentParentInfo;

#[tokio::test]
async fn async_agent_spawn_links_the_child_session_on_the_parent_tool_item() {
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, session_manager) = test_persistent_coordinator();
    let parent = session_manager
        .create_session(
            "Async delegation parent".to_string(),
            "Ultimate".to_string(),
            SessionConfig {
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("create parent session");
    let child = session_manager
        .create_session(
            "Async delegation child".to_string(),
            "GeneralPurpose".to_string(),
            SessionConfig {
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                model_id: Some("child-subagent-model".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("create spawned child session");
    let parent_turn_id = session_manager
        .start_dialog_turn(
            &parent.session_id,
            "Ultimate".to_string(),
            "delegate the audit".to_string(),
            Some("turn-async-agent-spawn".to_string()),
            None,
            None,
        )
        .await
        .expect("start parent turn");
    let tool_call_id = "tool-call-agent-spawn".to_string();
    let spawning_round = Message::assistant_with_tools(
        String::new(),
        vec![ToolCall {
            tool_id: tool_call_id.clone(),
            tool_name: "AgentSpawn".to_string(),
            arguments: serde_json::json!({
                "prompt": "audit the widget",
                "agent_type": "GeneralPurpose",
                "run_in_background": true,
            }),
            raw_arguments: None,
            is_error: false,
            parse_error: None,
            recovered_from_truncation: false,
            repair_kind: Default::default(),
        }],
    )
    .with_turn_id(parent_turn_id.clone())
    .with_round_id("round-async-agent-spawn".to_string());
    let started_result = Message::tool_result(crate::agentic::core::ToolResult {
        tool_id: tool_call_id.clone(),
        tool_name: "AgentSpawn".to_string(),
        effective_tool_name: None,
        result: serde_json::json!({
            "context_mode": "isolated",
            "status": "started",
            "run_in_background": true,
            "bg_task_id": "bg-task-1",
            "agent_id": "agent-1",
        }),
        result_for_assistant: Some("Background agent started".to_string()),
        is_error: false,
        duration_ms: Some(4),
        image_attachments: None,
    })
    .with_turn_id(parent_turn_id.clone())
    .with_round_id("round-async-agent-spawn".to_string());
    session_manager
        .add_messages(&parent.session_id, vec![spawning_round, started_result])
        .await
        .expect("record the spawning round and its started result");

    // An asynchronous spawn returns while the parent Turn is still running,
    // so the link has to reach the parent Turn's persisted tool item.
    coordinator
        .stamp_subagent_link_on_parent_tool_item(
            &SubagentParentInfo {
                tool_call_id: tool_call_id.clone(),
                session_id: parent.session_id.clone(),
                dialog_turn_id: parent_turn_id.clone(),
            },
            &child.session_id,
            "turn-child-subagent",
        )
        .await;

    let storage = session_manager
        .effective_session_storage_path(&parent.session_id)
        .await
        .expect("parent session storage path");
    let turns = coordinator
        .load_relay_session_turns(&storage, &parent.session_id, Some(&parent_turn_id))
        .await
        .expect("relay read of the running parent turn");
    let item = turns
        .iter()
        .flat_map(|turn| turn.model_rounds.iter())
        .flat_map(|round| round.tool_items.iter())
        .find(|item| item.id == tool_call_id)
        .expect("the spawning tool item survives the relay read");
    assert_eq!(
        item.tool_name, "AgentSpawn",
        "the link belongs to the spawning tool item"
    );
    assert_eq!(
        item.subagent_session_id.as_deref(),
        Some(child.session_id.as_str())
    );
    assert_eq!(
        item.subagent_dialog_turn_id.as_deref(),
        Some("turn-child-subagent")
    );
    assert_eq!(
        item.subagent_model_id.as_deref(),
        Some("child-subagent-model")
    );
    // The record stream serializes the persisted item as it is, so a remote
    // consumer reads the link under its camelCase wire name.
    assert_eq!(
        serde_json::to_value(item).expect("item serializes")["subagentSessionId"],
        serde_json::json!(child.session_id)
    );

    session_manager
        .delete_session_by_id(&parent.session_id)
        .await
        .expect("clean up persisted parent session");
    session_manager
        .delete_session_by_id(&child.session_id)
        .await
        .expect("clean up persisted child session");
}

#[test]
fn hidden_subagent_dialog_turn_id_reuses_existing_or_generates_raw_uuid() {
    let mut missing = None;
    let generated = super::super::ensure_hidden_subagent_dialog_turn_id(&mut missing);

    assert_eq!(missing.as_deref(), Some(generated.as_str()));
    assert!(uuid::Uuid::parse_str(&generated).is_ok());
    assert!(!generated.starts_with("subagent-"));

    let mut existing = Some("child-turn".to_string());
    assert_eq!(
        super::super::ensure_hidden_subagent_dialog_turn_id(&mut existing),
        "child-turn"
    );
    assert_eq!(existing.as_deref(), Some("child-turn"));
}

#[tokio::test]
async fn model_facing_subagent_ids_are_stable_and_parent_scoped() {
    let (coordinator, _) = test_coordinator();

    let first_agent = coordinator
        .agent_id_for_subagent_session_with_requested_id("parent-1", "subagent-session-1", None)
        .await
        .expect("allocate first agent id");
    let repeated_agent = coordinator
        .agent_id_for_subagent_session_with_requested_id("parent-1", "subagent-session-1", None)
        .await
        .expect("reuse first agent id");
    let second_agent = coordinator
        .agent_id_for_subagent_session_with_requested_id("parent-1", "subagent-session-2", None)
        .await
        .expect("allocate second agent id");
    let other_parent_agent = coordinator
        .agent_id_for_subagent_session_with_requested_id("parent-2", "subagent-session-3", None)
        .await
        .expect("allocate agent id for another parent");

    assert_eq!(first_agent, "a1");
    assert_eq!(repeated_agent, "a1");
    assert_eq!(second_agent, "a2");
    assert_eq!(other_parent_agent, "a1");
    assert_eq!(
        coordinator
            .resolve_agent_id("parent-1", "a2")
            .await
            .expect("resolve agent id"),
        "subagent-session-2"
    );

    let first_bg_task = register_test_background_task(
        &coordinator,
        "parent-1",
        "parent-turn-1",
        "subagent-session-1",
    )
    .await;
    let second_bg_task = register_test_background_task(
        &coordinator,
        "parent-1",
        "parent-turn-2",
        "subagent-session-1",
    )
    .await;
    assert_eq!(first_bg_task.bg_task_id, "a1_bg1");
    assert_eq!(second_bg_task.bg_task_id, "a1_bg2");

    let custom = coordinator
        .background_subagent_outcomes
        .register(BackgroundTaskRegistration {
            parent_session_id: "parent-1".to_string(),
            requested_agent_id: Some("reviewer".to_string()),
            child_session_id: "reviewer-session".to_string(),
            parent_dialog_turn_id: "parent-turn-3".to_string(),
            parent_tool_call_id: "tool-reviewer".to_string(),
            child_dialog_turn_id: "turn-reviewer".to_string(),
        })
        .await
        .expect("register caller-named agent");
    assert_eq!(custom.agent_id, "reviewer");
    assert_eq!(custom.bg_task_id, "reviewer_bg1");
    assert_eq!(
        coordinator
            .resolve_agent_id("parent-1", "reviewer")
            .await
            .expect("resolve caller-named agent"),
        "reviewer-session"
    );
}

#[test]
fn external_subagent_surfaces_use_logical_id_instead_of_runtime_generation_key() {
    let runtime_type = "external_subagent_runtime:generation-hash";
    let logical_type = logical_subagent_type_or_runtime(Some("Reviewer"), runtime_type);

    assert_eq!(logical_type, "Reviewer");
    let relationship = build_subagent_session_relationship(
        None,
        &logical_type,
        SessionContinuationPolicy::FreshOnly,
    );
    assert_eq!(relationship.subagent_type.as_deref(), Some("Reviewer"));
    assert_eq!(
        relationship.continuation_policy,
        Some(SessionContinuationPolicy::FreshOnly)
    );
}

#[test]
fn external_delegation_agent_ids_are_semantic_unique_and_valid() {
    let first = external_delegation_agent_id(
        "Review Security / Auth",
        &uuid::Uuid::parse_str("12345678-1234-5678-1234-567812345678").unwrap(),
    );
    let second = external_delegation_agent_id(
        "Review Security / Auth",
        &uuid::Uuid::parse_str("87654321-1234-5678-1234-567812345678").unwrap(),
    );

    assert_eq!(first, "review-security-auth-12345678");
    assert_eq!(second, "review-security-auth-87654321");
    assert_ne!(first, second);
    crate::agentic::coordination::validate_agent_id(&first).unwrap();
    crate::agentic::coordination::validate_agent_id(&second).unwrap();
}
