use super::super::*;
use openbitfun_product_domains::external_sources::ExternalSourceScope;

#[tokio::test]
async fn external_subagent_model_bindings_share_the_existing_atomic_preference_owner() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("external-sources.json");
    let process_a = ExternalSourcePreferenceStore::new(path.clone());
    let process_b = ExternalSourcePreferenceStore::new(path);
    let workspace_a = "external_subagent_model_binding:workspace-a";
    let workspace_b = "external_subagent_model_binding:workspace-b";

    let first = persist_subagent_model_binding_with_store(
        &process_a,
        workspace_a,
        Some(ExternalSubagentModelBindingTarget::Primary),
        0,
    )
    .await
    .unwrap();
    assert_eq!(first.preference_revision, 1);
    assert_eq!(
        first.subagent_model_bindings.get(workspace_a),
        Some(&ExternalSubagentModelBindingTarget::Primary)
    );

    let merged = persist_subagent_model_binding_with_store(
        &process_b,
        workspace_b,
        Some(ExternalSubagentModelBindingTarget::Model {
            model_id: "glm-project".to_string(),
        }),
        first.preference_revision,
    )
    .await
    .unwrap();
    assert_eq!(merged.preference_revision, 2);
    assert_eq!(merged.subagent_model_bindings.len(), 2);

    let error = persist_subagent_model_binding_with_store(
        &process_a,
        workspace_a,
        Some(ExternalSubagentModelBindingTarget::Fast),
        first.preference_revision,
    )
    .await
    .expect_err("a stale process must not overwrite a newer workspace binding");
    assert_eq!(
        ExternalSourceOperationError::decode(&error)
            .expect("stale binding writes use the typed error contract")
            .code,
        ExternalSourceOperationErrorCode::StaleRevision
    );

    let replaced = persist_subagent_model_binding_with_store(
        &process_a,
        workspace_a,
        Some(ExternalSubagentModelBindingTarget::Fast),
        merged.preference_revision,
    )
    .await
    .unwrap();
    assert_eq!(replaced.preference_revision, 3);
    assert_eq!(
        replaced.subagent_model_bindings.get(workspace_a),
        Some(&ExternalSubagentModelBindingTarget::Fast)
    );

    let cleared = persist_subagent_model_binding_with_store(
        &process_b,
        workspace_a,
        None,
        replaced.preference_revision,
    )
    .await
    .unwrap();
    assert_eq!(cleared.preference_revision, 4);
    assert!(!cleared.subagent_model_bindings.contains_key(workspace_a));
    assert!(cleared.subagent_model_bindings.contains_key(workspace_b));
}

#[test]
fn external_subagent_model_binding_mutation_fails_closed_against_the_current_snapshot() {
    let binding_key = "external_subagent_model_binding:known";
    let mut snapshot = ExternalSourceCatalogSnapshot {
        generation: 0,
        discovery_pending: false,
        sources: Vec::new(),
        commands: Vec::new(),
        command_conflicts: Vec::new(),
        tools: Vec::new(),
        tool_approval_requests: Vec::new(),
        tool_conflicts: Vec::new(),
        mcp_generation: 0,
        mcp_servers: Vec::new(),
        mcp_approval_requests: Vec::new(),
        mcp_conflicts: Vec::new(),
        subagent_generation: 7,
        preference_revision: 11,
        subagents: Vec::new(),
        subagent_model_binding_groups: vec![ExternalSubagentModelBindingGroup {
            binding_key: binding_key.to_string(),
            request: ExternalSubagentModelRequest::Reference {
                provider_hint: Some("openai".to_string()),
                model_name: "gpt-project".to_string(),
            },
            profile_request: None,
            scope: ExternalSourceScope::Project,
            method: ExternalSubagentModelBindingMethod::BindingRequired,
            selected_target: None,
            effective_model_label: None,
            affected_candidate_ids: vec!["review".to_string()],
        }],
        subagent_model_binding_options: vec![ExternalSubagentModelBindingOption {
            target: ExternalSubagentModelBindingTarget::Primary,
            effective_model_label: "Primary model".to_string(),
            configured_reasoning_effort: None,
        }],
        subagent_conflicts: Vec::new(),
        pending_subagent_approvals: Vec::new(),
        integration_policy: Default::default(),
        diagnostics: Vec::new(),
    };

    assert!(validate_subagent_model_binding_mutation(
        &snapshot,
        binding_key,
        Some(&ExternalSubagentModelBindingTarget::Primary),
        7,
        11,
    )
    .is_ok());
    assert!(validate_subagent_model_binding_mutation(&snapshot, binding_key, None, 7, 11,).is_ok());

    for (key, target, generation, revision, expected_code) in [
        (
            "external_subagent_model_binding:missing",
            None,
            7,
            11,
            ExternalSourceOperationErrorCode::NotFound,
        ),
        (
            binding_key,
            Some(&ExternalSubagentModelBindingTarget::Fast),
            7,
            11,
            ExternalSourceOperationErrorCode::Unavailable,
        ),
        (
            binding_key,
            None,
            8,
            11,
            ExternalSourceOperationErrorCode::StaleRevision,
        ),
        (
            binding_key,
            None,
            7,
            12,
            ExternalSourceOperationErrorCode::StaleRevision,
        ),
    ] {
        let error =
            validate_subagent_model_binding_mutation(&snapshot, key, target, generation, revision)
                .expect_err("invalid binding mutations must fail closed");
        assert_eq!(
            ExternalSourceOperationError::decode(&error).unwrap().code,
            expected_code
        );
    }

    snapshot.subagent_model_binding_groups[0].method = ExternalSubagentModelBindingMethod::Exact;
    let error = validate_subagent_model_binding_mutation(
        &snapshot,
        binding_key,
        Some(&ExternalSubagentModelBindingTarget::Primary),
        7,
        11,
    )
    .expect_err("exact source matches are read-only");
    assert_eq!(
        ExternalSourceOperationError::decode(&error).unwrap().code,
        ExternalSourceOperationErrorCode::Unavailable
    );
}

#[test]
fn only_model_configuration_events_refresh_external_model_bindings() {
    assert!(config_update_refreshes_external_model_bindings(
        &ConfigUpdateEvent::ModelConfigurationUpdated
    ));
    assert!(!config_update_refreshes_external_model_bindings(
        &ConfigUpdateEvent::AppearanceUpdated {
            appearance_id: "openbitfun-dark".to_string(),
        }
    ));
}
