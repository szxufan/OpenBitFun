use super::super::*;
use super::support::test_service;

#[tokio::test]
async fn subagent_conflict_history_advances_revision_and_rejects_stale_process_actions() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("external-sources.json");
    let process_a = ExternalSourcePreferenceStore::new(path.clone());
    let process_b = ExternalSourcePreferenceStore::new(path);
    let lineage = "external_subagent_lineage:local-user:workspace:review";
    let conflict_v1 = "external_subagent:local-user:workspace:review:v1";
    let conflict_v2 = "external_subagent:local-user:workspace:review:v2";

    let (changed, observed_v1) = persist_observed_subagent_conflicts_with_store(
        &process_a,
        &BTreeMap::from([(lineage.to_string(), conflict_v1.to_string())]),
    )
    .await
    .unwrap();
    assert!(changed);
    assert_eq!(observed_v1.preference_revision, 1);

    let selected_v1 = persist_subagent_conflict_choice_with_store(
        &process_a,
        conflict_v1,
        "external_subagent:candidate-v1",
        None,
        observed_v1.preference_revision,
    )
    .await
    .unwrap();
    assert_eq!(selected_v1.preference_revision, 2);

    let (changed, observed_v2) = persist_observed_subagent_conflicts_with_store(
        &process_b,
        &BTreeMap::from([(lineage.to_string(), conflict_v2.to_string())]),
    )
    .await
    .unwrap();
    assert!(changed);
    assert_eq!(observed_v2.preference_revision, 3);
    assert!(!observed_v2
        .subagent_conflict_choices
        .contains_key(conflict_v1));
    assert_eq!(
        observed_v2.subagent_conflict_choices[conflict_v2],
        SUBAGENT_CONFLICT_RESELECTION_REQUIRED
    );

    let error = persist_subagent_conflict_choice_with_store(
        &process_a,
        conflict_v1,
        "external_subagent:candidate-v1",
        None,
        selected_v1.preference_revision,
    )
    .await
    .expect_err("the stale process must not overwrite the new conflict generation");
    assert_eq!(
        ExternalSourceOperationError::decode(&error)
            .expect("stale conflict actions use the typed error contract")
            .code,
        ExternalSourceOperationErrorCode::StaleRevision
    );
}

#[test]
fn observed_tool_conflict_requires_reselection_after_external_lineage_changes() {
    let old = "external_tool:domain:read:old";
    let current = "external_tool:domain:read:new";
    let mut choices = BTreeMap::from([(old.to_string(), "external:source-a".to_string())]);

    reconcile_observed_tool_conflict(&mut choices, current);

    assert!(!choices.contains_key(old));
    assert_eq!(
        choices.get(current).map(String::as_str),
        Some(TOOL_CONFLICT_RESELECTION_REQUIRED)
    );
}

#[test]
fn first_observed_tool_conflict_persists_an_unresolved_lineage() {
    let conflict_key = "external_tool:domain:read:first";
    let mut choices = BTreeMap::new();

    reconcile_observed_tool_conflict(&mut choices, conflict_key);

    assert_eq!(
        choices.get(conflict_key).map(String::as_str),
        Some(UNRESOLVED_TOOL_CONFLICT_CHOICE)
    );
}

#[test]
fn conflict_lineages_are_compact_and_independent() {
    let mut choices = BTreeMap::from([
        (
            "prompt_command:local-user:review:old".to_string(),
            "external-a".to_string(),
        ),
        (
            "native:prompt_command:local-user:help:old".to_string(),
            "openbitfun.cli:help".to_string(),
        ),
    ]);
    let mut lineage_keys = BTreeMap::from([
        (
            "prompt_command:local-user:review".to_string(),
            "prompt_command:local-user:review:old".to_string(),
        ),
        (
            "native:prompt_command:local-user:help".to_string(),
            "native:prompt_command:local-user:help:old".to_string(),
        ),
    ]);
    let mut conflicted_ids = BTreeSet::from([
        "external-a".to_string(),
        "external-b".to_string(),
        "openbitfun.cli:help".to_string(),
    ]);

    ExternalSourceCoordinator::reconcile_conflict_preferences(
        &mut choices,
        &mut lineage_keys,
        &mut conflicted_ids,
        "native:prompt_command:local-user:help:new",
        "openbitfun.cli:help",
        &["openbitfun.cli:help".to_string()],
    );

    assert!(choices.contains_key("prompt_command:local-user:review:old"));
    assert!(!choices.contains_key("native:prompt_command:local-user:help:old"));
    assert_eq!(choices.len(), 2);
    assert_eq!(lineage_keys.len(), 2);
}

#[test]
fn tool_decisions_keep_only_the_current_decline_per_approval() {
    let mut config = ExternalSourcesConfig::default();

    reconcile_tool_target_decision(
        &mut config,
        "approval-a".to_string(),
        "decision-v1".to_string(),
        false,
    );
    reconcile_tool_target_decision(
        &mut config,
        "approval-a".to_string(),
        "decision-v2".to_string(),
        false,
    );

    assert_eq!(
        config.declined_tool_decisions,
        BTreeMap::from([("approval-a".to_string(), "decision-v2".to_string())])
    );
    reconcile_tool_target_decision(
        &mut config,
        "approval-a".to_string(),
        "decision-v2".to_string(),
        true,
    );
    assert!(config.declined_tool_decisions.is_empty());
    assert_eq!(
        config.approved_tool_targets,
        BTreeSet::from(["approval-a".to_string()])
    );
}

#[test]
fn capability_bulk_decisions_update_multiple_items_with_one_revision() {
    let mut config = ExternalSourcesConfig {
        preference_revision: 7,
        ..Default::default()
    };

    assert!(apply_capability_bulk_decisions(
        &mut config,
        ExternalCapabilityBulkDecisionSet::Tool(vec![
            ("tool-approval-a".to_string(), "tool-decision-a".to_string()),
            ("tool-approval-b".to_string(), "tool-decision-b".to_string()),
        ]),
        true,
    ));
    assert_eq!(config.preference_revision, 8);
    assert_eq!(
        config.approved_tool_targets,
        BTreeSet::from(["tool-approval-a".to_string(), "tool-approval-b".to_string(),])
    );

    assert!(!apply_capability_bulk_decisions(
        &mut config,
        ExternalCapabilityBulkDecisionSet::Tool(vec![
            ("tool-approval-a".to_string(), "tool-decision-a".to_string()),
            ("tool-approval-b".to_string(), "tool-decision-b".to_string()),
        ]),
        true,
    ));
    assert_eq!(config.preference_revision, 8);

    assert!(apply_capability_bulk_decisions(
        &mut config,
        ExternalCapabilityBulkDecisionSet::Mcp(vec![
            "external_mcp_approval:local-user:workspace:docs:v1".to_string(),
            "external_mcp_approval:local-user:workspace:search:v1".to_string(),
        ]),
        false,
    ));
    assert_eq!(config.preference_revision, 9);
    assert!(config
        .mcp_server_decisions
        .values()
        .all(|decision| !decision.approved));

    assert!(apply_capability_bulk_decisions(
        &mut config,
        ExternalCapabilityBulkDecisionSet::Subagent(vec![
            "agent-decision-a".to_string(),
            "agent-decision-b".to_string(),
        ]),
        true,
    ));
    assert_eq!(config.preference_revision, 10);
    assert_eq!(
        config.approved_subagent_envelopes,
        BTreeSet::from([
            "agent-decision-a".to_string(),
            "agent-decision-b".to_string(),
        ])
    );
    assert!(apply_capability_bulk_decisions(
        &mut config,
        ExternalCapabilityBulkDecisionSet::Subagent(vec![
            "agent-decision-a".to_string(),
            "agent-decision-b".to_string(),
        ]),
        false,
    ));
    assert_eq!(config.preference_revision, 11);
    assert!(config.approved_subagent_envelopes.is_empty());
}

#[tokio::test]
async fn capability_bulk_actions_reject_catalog_drift_without_expanding_the_decision_set() {
    let service = test_service(Vec::new());
    {
        let mut snapshot = lock_snapshot(&service.snapshot);
        snapshot.generation = 12;
        snapshot.subagent_generation = 13;
        snapshot.mcp_generation = 14;
        snapshot.preference_revision = 7;
    }

    let errors = [
        service
            .set_tool_targets_enabled(Vec::new(), true, 11, 7)
            .await
            .expect_err("tool catalog drift must require a new review"),
        service
            .set_subagents_enabled(Vec::new(), true, 12, 7)
            .await
            .expect_err("subagent catalog drift must require a new review"),
        service
            .set_mcp_servers_enabled(Vec::new(), true, 13, 7)
            .await
            .expect_err("MCP catalog drift must require a new review"),
    ];

    for error in errors {
        assert_eq!(
            ExternalSourceOperationError::decode(&error)
                .expect("bulk catalog drift uses the typed error contract")
                .code,
            ExternalSourceOperationErrorCode::StaleRevision
        );
    }
}

#[tokio::test]
async fn tool_approval_waits_for_refresh_and_rejects_a_changed_decision() {
    let service = test_service(Vec::new());
    let request = |decision_key: &str, content_version: &str| {
        serde_json::from_value::<ExternalToolApprovalRequest>(serde_json::json!({
            "approvalKey": "approval-a",
            "decisionKey": decision_key,
            "targetId": {
                "source": { "providerId": "opencode.tools", "sourceId": "project" },
                "localId": "review.js"
            },
            "sourceDisplayName": "OpenCode project tools",
            "sourceScope": "project",
            "sourceLocation": "/repo/.opencode/tools/review.js",
            "workingDirectory": "/repo",
            "runtimeKind": "java_script",
            "capabilities": ["file_system"],
            "contentVersion": content_version,
            "toolNames": ["review"]
        }))
        .unwrap()
    };
    lock_snapshot(&service.snapshot).tool_approval_requests = vec![request("decision-v1", "v1")];
    let expected_preference_revision = lock_snapshot(&service.snapshot).preference_revision;

    let refresh_guard = service.refresh_gate.lock().await;
    let decision_service = Arc::clone(&service);
    let decision = tokio::spawn(async move {
        decision_service
            .set_tool_target_decision(
                "approval-a",
                "decision-v1",
                true,
                expected_preference_revision,
            )
            .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        service.tool_decision_gate_waiting.notified(),
    )
    .await
    .expect("approval task must reach the refresh gate");
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            service.tool_decision_gate_acquired.notified(),
        )
        .await
        .is_err(),
        "approval must not enter the decision critical section while refresh owns the gate"
    );

    lock_snapshot(&service.snapshot).tool_approval_requests = vec![request("decision-v2", "v2")];
    drop(refresh_guard);
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        service.tool_decision_gate_acquired.notified(),
    )
    .await
    .expect("approval task must enter after the refresh releases the gate");

    let error = decision
        .await
        .unwrap()
        .expect_err("the approval must not apply to the changed content");
    assert_eq!(
        ExternalSourceOperationError::decode(&error)
            .expect("changed tool decisions use the typed error contract")
            .code,
        ExternalSourceOperationErrorCode::NotFound
    );
}

#[test]
fn tool_conflict_choices_keep_only_the_current_version_per_lineage() {
    let mut choices = BTreeMap::from([
        (
            "external_tool:local-user:review:old".to_string(),
            "external-a".to_string(),
        ),
        (
            "external_tool:local-user:help:old".to_string(),
            "builtin-help".to_string(),
        ),
    ]);

    reconcile_versioned_tool_conflict_choice(
        &mut choices,
        "external_tool:local-user:review:new".to_string(),
        "external-b".to_string(),
    );

    assert!(!choices.contains_key("external_tool:local-user:review:old"));
    assert_eq!(choices["external_tool:local-user:review:new"], "external-b");
    assert_eq!(choices["external_tool:local-user:help:old"], "builtin-help");
    assert_eq!(choices.len(), 2);
}
