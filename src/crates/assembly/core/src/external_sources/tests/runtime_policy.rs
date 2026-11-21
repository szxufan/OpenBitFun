use super::super::*;
use super::support::{delayed_provider, test_service, CountingExternalMcpRuntime};
use openbitfun_product_domains::external_sources::{
    EcosystemId, ExternalSourceRecord, ExternalSourceScope,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn safe_mode_keeps_commands_visible_and_retires_managed_executables() {
    let discovery_calls = Arc::new(AtomicUsize::new(0));
    let runtime = Arc::new(CountingExternalMcpRuntime::default());
    let workspace = tempfile::tempdir().unwrap();
    let mut service = test_service(vec![delayed_provider(
        "opencode",
        std::time::Duration::ZERO,
        discovery_calls,
    )]);
    let service_inner = Arc::get_mut(&mut service).expect("test owns the service");
    service_inner.workspace_id = Some(uuid::Uuid::new_v4().to_string());
    service_inner.workspace_root = Some(workspace.path().to_path_buf());
    service_inner.mcp_runtime = runtime.clone();

    let requests = lock_coordinator(&service.control_plane).discovery_requests();
    let batch = service
        .control_plane
        .discover_commands(requests, std::time::Duration::from_secs(1))
        .await;
    assert!(batch.deferred.is_empty());
    let mut catalog =
        lock_coordinator(&service.control_plane).apply_discovery_results(batch.immediate);
    catalog.integration_policy = service.snapshot().integration_policy;
    *lock_snapshot(&service.snapshot) = catalog;
    service
        .active_mcp_runtime_ids
        .lock()
        .await
        .insert("managed-mcp".to_string());

    let snapshot = service.set_safe_mode(true, None).await.unwrap();
    let surface = service.surface_snapshot(ExternalSourceHostCapabilities::read_write());

    assert!(surface.control.safe_mode);
    assert!(!surface.catalog.commands.is_empty());
    assert!(snapshot
        .tools
        .iter()
        .all(|tool| !matches!(tool.activation, ExternalToolActivationState::Active)));
    assert!(snapshot
        .mcp_servers
        .iter()
        .all(|server| server.runtime_id.is_none()));
    assert!(snapshot.subagents.iter().all(|agent| !matches!(
        agent.activation_state,
        ExternalSubagentActivationState::Active
    )));
    assert!(service.active_mcp_runtime_ids.lock().await.is_empty());
    assert!(runtime.calls.load(Ordering::SeqCst) >= 2);

    service.set_safe_mode(false, None).await.unwrap();
    assert!(
        !service
            .surface_snapshot(ExternalSourceHostCapabilities::read_write())
            .control
            .safe_mode
    );
}

#[tokio::test]
async fn control_action_returns_typed_stale_revision_and_shared_surface() {
    let workspace = tempfile::tempdir().unwrap();
    let mut service = test_service(Vec::new());
    Arc::get_mut(&mut service)
        .expect("test owns the service")
        .workspace_root = Some(workspace.path().to_path_buf());
    Arc::get_mut(&mut service).unwrap().workspace_id = Some(uuid::Uuid::new_v4().to_string());
    let revision = read_external_sources_config()
        .await
        .unwrap()
        .preference_revision;
    lock_snapshot(&service.snapshot).preference_revision = revision;
    let stale_request = ExternalSourceControlRequestV1 {
        schema_version: EXTERNAL_SOURCE_CONTROL_SCHEMA_V1,
        operation_id: "safe-mode-stale".to_string(),
        expected_preference_revision: Some(revision.saturating_add(1)),
        action: ExternalSourceControlActionV1::SetSafeMode { enabled: true },
    };

    let error = service
        .apply_control_action(stale_request)
        .await
        .unwrap_err();

    assert_eq!(error.code, ExternalSourceOperationErrorCode::StaleRevision);
    assert_eq!(error.correlation_id.as_deref(), Some("safe-mode-stale"));
    assert_eq!(
            error.recovery_actions,
            vec![
                openbitfun_product_domains::external_source_control::ExternalSourceRecoveryActionV1::Refresh,
            ]
        );
    assert_eq!(
            error.stage,
            Some(
                openbitfun_product_domains::external_source_control::ExternalSourceOperationStage::ApplyPreference
            )
        );

    let response = service
        .apply_control_action(ExternalSourceControlRequestV1 {
            schema_version: EXTERNAL_SOURCE_CONTROL_SCHEMA_V1,
            operation_id: "safe-mode-on".to_string(),
            expected_preference_revision: Some(revision),
            action: ExternalSourceControlActionV1::SetSafeMode { enabled: true },
        })
        .await
        .unwrap();
    assert!(response.control.safe_mode);
    assert_eq!(
        response.control.refresh_generation,
        response.catalog.generation
    );

    service.set_safe_mode(false, None).await.unwrap();
}

#[tokio::test]
async fn acknowledging_an_ecosystem_survives_a_reload_and_stays_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("external-sources.json");
    let store = ExternalSourcePreferenceStore::new(path.clone());
    let key = acknowledged_ecosystem_key(LEGACY_LOCAL_EXECUTION_DOMAIN_ID, "opencode");

    store
        .update(|config| {
            config.acknowledged_ecosystems.insert(key.clone());
        })
        .await
        .unwrap();
    store
        .update(|config| {
            config.acknowledged_ecosystems.insert(key.clone());
        })
        .await
        .unwrap();

    // A fresh store proves the record came back from disk, not from memory.
    let reloaded = ExternalSourcePreferenceStore::new(path)
        .read()
        .await
        .unwrap();
    assert_eq!(reloaded.acknowledged_ecosystems, BTreeSet::from([key]));
    // Awareness is not a policy decision, so it must not consume a revision.
    assert_eq!(reloaded.preference_revision, 0);
}

#[tokio::test]
async fn acknowledgement_is_scoped_to_its_execution_domain() {
    let temp = tempfile::tempdir().unwrap();
    let store = ExternalSourcePreferenceStore::new(temp.path().join("external-sources.json"));
    let local = acknowledged_ecosystem_key(LEGACY_LOCAL_EXECUTION_DOMAIN_ID, "opencode");
    let remote = acknowledged_ecosystem_key("remote-host", "opencode");

    store
        .update(|config| {
            config.acknowledged_ecosystems.insert(local.clone());
        })
        .await
        .unwrap();

    let persisted = store.read().await.unwrap();
    assert!(persisted.acknowledged_ecosystems.contains(&local));
    assert!(!persisted.acknowledged_ecosystems.contains(&remote));
}

#[test]
fn acknowledgement_keys_never_collide_across_domains_or_ecosystems() {
    assert_ne!(
        acknowledged_ecosystem_key("local-user", "opencode"),
        acknowledged_ecosystem_key("local-user", "codex")
    );
    assert_ne!(
        acknowledged_ecosystem_key("local-user", "opencode"),
        acknowledged_ecosystem_key("remote-host", "opencode")
    );
}

#[test]
fn opencode_registry_owns_low_friction_defaults_and_safety_ceilings() {
    let mut config = ExternalSourcesConfig::default();
    config
        .integration_policy
        .known_mut()
        .expect("the built-in policy schema is known")
        .user_defaults
        .enabled = true;
    let policy = integration_policy_snapshot(&config, None).expect("built-in policy is valid");
    let descriptor = policy
        .registered_ecosystems
        .iter()
        .find(|descriptor| descriptor.ecosystem_id.as_str() == OPENCODE_ECOSYSTEM_ID)
        .expect("OpenCode is registered by product assembly");

    for (capability_id, recommended, ceiling) in [
        (
            EXTERNAL_CAPABILITY_COMMAND,
            ExternalIntegrationAccess::Auto,
            ExternalIntegrationAccess::Auto,
        ),
        (
            EXTERNAL_CAPABILITY_TOOL,
            ExternalIntegrationAccess::AskBeforeUse,
            ExternalIntegrationAccess::AskBeforeUse,
        ),
        (
            EXTERNAL_CAPABILITY_SUBAGENT,
            ExternalIntegrationAccess::AskBeforeUse,
            ExternalIntegrationAccess::AskBeforeUse,
        ),
        (
            EXTERNAL_CAPABILITY_MCP,
            ExternalIntegrationAccess::AskBeforeUse,
            ExternalIntegrationAccess::AskBeforeUse,
        ),
        (
            EXTERNAL_CAPABILITY_REFERENCE,
            ExternalIntegrationAccess::Auto,
            ExternalIntegrationAccess::Auto,
        ),
    ] {
        let capability = descriptor
            .capabilities
            .iter()
            .find(|capability| capability.capability_id.as_str() == capability_id)
            .expect("built-in capability is registered");
        assert_eq!(capability.recommended_access, recommended);
        assert_eq!(capability.safety_ceiling, ceiling);
        assert_eq!(
            integration_access(&policy, OPENCODE_ECOSYSTEM_ID, capability_id),
            recommended
        );
    }
}

#[test]
fn default_registry_exposes_only_each_ecosystems_supported_asset_kinds() {
    let registrations = default_external_integration_registry();
    assert_eq!(registrations.len(), 4);

    let expected = BTreeMap::from([
        (
            "deepseek-harness",
            BTreeSet::from([EXTERNAL_CAPABILITY_MCP]),
        ),
        (
            "opencode",
            BTreeSet::from([
                EXTERNAL_CAPABILITY_COMMAND,
                EXTERNAL_CAPABILITY_TOOL,
                EXTERNAL_CAPABILITY_SUBAGENT,
                EXTERNAL_CAPABILITY_MCP,
                EXTERNAL_CAPABILITY_REFERENCE,
            ]),
        ),
        (
            "claude-code",
            BTreeSet::from([
                EXTERNAL_CAPABILITY_COMMAND,
                EXTERNAL_CAPABILITY_SUBAGENT,
                EXTERNAL_CAPABILITY_MCP,
            ]),
        ),
        (
            "codex",
            BTreeSet::from([EXTERNAL_CAPABILITY_SUBAGENT, EXTERNAL_CAPABILITY_MCP]),
        ),
    ]);

    for registration in registrations {
        registration
            .validate()
            .expect("built-in registration is valid");
        let ecosystem = registration.descriptor.ecosystem_id.as_str();
        let capabilities = registration
            .descriptor
            .capabilities
            .iter()
            .map(|capability| capability.capability_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(capabilities, expected[ecosystem]);
        for capability in &registration.descriptor.capabilities {
            let expected_access = if ecosystem == "opencode"
                && matches!(
                    capability.capability_id.as_str(),
                    EXTERNAL_CAPABILITY_COMMAND | EXTERNAL_CAPABILITY_REFERENCE
                )
                || ecosystem == "claude-code"
                    && capability.capability_id.as_str() == EXTERNAL_CAPABILITY_COMMAND
            {
                ExternalIntegrationAccess::Auto
            } else {
                ExternalIntegrationAccess::AskBeforeUse
            };
            assert_eq!(capability.recommended_access, expected_access);
            assert_eq!(capability.safety_ceiling, expected_access);
        }
    }
}

#[test]
fn active_capability_sets_are_scoped_per_ecosystem_for_every_asset_kind() {
    let mut config = ExternalSourcesConfig::default();
    config
        .integration_policy
        .known_mut()
        .expect("the built-in policy schema is known")
        .user_defaults
        .enabled = true;
    let mut policy = integration_policy_snapshot(&config, None).expect("built-in policy is valid");
    let template_descriptor = policy.registered_ecosystems[0].clone();
    let template_effective = policy
        .effective
        .ecosystems
        .get(&template_descriptor.ecosystem_id)
        .cloned()
        .expect("built-in effective ecosystem exists");
    let discover_id = EcosystemId::new("discover.ecosystem").unwrap();
    let active_id = EcosystemId::new("active.ecosystem").unwrap();
    let mut discover_descriptor = template_descriptor.clone();
    discover_descriptor.ecosystem_id = discover_id.clone();
    discover_descriptor.display_name = "Discover ecosystem".to_string();
    let mut active_descriptor = template_descriptor;
    active_descriptor.ecosystem_id = active_id.clone();
    active_descriptor.display_name = "Active ecosystem".to_string();
    policy.registered_ecosystems = vec![discover_descriptor, active_descriptor];

    let mut discover_effective = template_effective.clone();
    discover_effective.ecosystem_id = discover_id.clone();
    for access in discover_effective.capabilities.values_mut() {
        *access = ExternalIntegrationAccess::DiscoverOnly;
    }
    let mut active_effective = template_effective;
    active_effective.ecosystem_id = active_id.clone();
    policy.effective.ecosystems = BTreeMap::from([
        (discover_id.clone(), discover_effective),
        (active_id.clone(), active_effective),
    ]);

    for capability in [
        EXTERNAL_CAPABILITY_COMMAND,
        EXTERNAL_CAPABILITY_TOOL,
        EXTERNAL_CAPABILITY_SUBAGENT,
        EXTERNAL_CAPABILITY_MCP,
        EXTERNAL_CAPABILITY_REFERENCE,
    ] {
        assert_eq!(
            ecosystems_with_discoverable_capability(&policy, capability),
            BTreeSet::from([discover_id.clone(), active_id.clone()])
        );
        assert_eq!(
            ecosystems_with_active_capability(&policy, capability),
            BTreeSet::from([active_id.clone()])
        );
    }
}

#[test]
fn ecosystem_registration_rejects_provider_and_capability_mismatches() {
    let mut incompatible_contract = default_external_integration_registry()
        .into_iter()
        .next()
        .expect("built-in registration exists");
    incompatible_contract.contract_major = EXTERNAL_ADAPTER_CONTRACT_MAJOR + 1;
    assert!(incompatible_contract
        .validate()
        .unwrap_err()
        .contains("contract major"));

    let mut wrong_ecosystem = default_external_integration_registry()
        .into_iter()
        .next()
        .expect("built-in registration exists");
    wrong_ecosystem.command_provider = Some(delayed_provider(
        "different.ecosystem",
        std::time::Duration::ZERO,
        Arc::new(AtomicUsize::new(0)),
    ));
    assert!(wrong_ecosystem
        .validate()
        .unwrap_err()
        .contains("different ecosystem"));

    let mut missing_provider = default_external_integration_registry()
        .into_iter()
        .next()
        .expect("built-in registration exists");
    missing_provider.command_provider = None;
    assert!(missing_provider
        .validate()
        .unwrap_err()
        .contains("provider registration do not match"));
}

#[test]
fn integration_policy_mutations_share_revision_and_keep_workspace_paths_private() {
    let temp = tempfile::tempdir().unwrap();
    let workspace_key = workspace_policy_key(Some("workspace-1")).expect("workspace has a key");
    assert!(workspace_key.starts_with("workspace:"));
    assert!(!workspace_key.contains(&temp.path().to_string_lossy().to_string()));

    let ecosystem_id =
        openbitfun_product_domains::external_sources::EcosystemId::new(OPENCODE_ECOSYSTEM_ID)
            .unwrap();
    let mut config = ExternalSourcesConfig::default();
    let user_mutation = ExternalIntegrationPolicyMutation {
        expected_preference_revision: 0,
        scope: ExternalIntegrationPolicyScope::User,
        change: ExternalIntegrationPolicyOperation::SetEcosystemMode {
            ecosystem_id: ecosystem_id.clone(),
            mode: ExternalIntegrationMode::DiscoverOnly,
        },
    };
    assert!(
        apply_integration_policy_mutation_to_config(&mut config, None, &user_mutation,).unwrap()
    );
    assert_eq!(config.preference_revision, 1);

    let stale = apply_integration_policy_mutation_to_config(&mut config, None, &user_mutation)
        .expect_err("old revisions cannot overwrite a newer policy");
    assert_eq!(
        ExternalSourceOperationError::decode(&stale)
            .expect("stale policy revisions use the typed error contract")
            .code,
        ExternalSourceOperationErrorCode::StaleRevision
    );

    let workspace_mutation = ExternalIntegrationPolicyMutation {
        expected_preference_revision: 1,
        scope: ExternalIntegrationPolicyScope::Workspace,
        change: ExternalIntegrationPolicyOperation::SetEcosystemMode {
            ecosystem_id: ecosystem_id.clone(),
            mode: ExternalIntegrationMode::Disabled,
        },
    };
    assert!(apply_integration_policy_mutation_to_config(
        &mut config,
        Some(&workspace_key),
        &workspace_mutation,
    )
    .unwrap());
    assert_eq!(config.preference_revision, 2);
    assert_eq!(
        config
            .integration_policy
            .known()
            .expect("the built-in policy schema is known")
            .workspace_overrides[&workspace_key]
            .ecosystems[&ecosystem_id]
            .mode,
        Some(ExternalIntegrationMode::Disabled)
    );
}

#[test]
fn invocation_authorization_uses_the_execution_domain_preference_key() {
    let source = ExternalSourceRecord {
        key: SourceKey::new("opencode", "global-tools").unwrap(),
        ecosystem_id: openbitfun_product_domains::external_sources::EcosystemId::new("opencode")
            .unwrap(),
        display_name: "OpenCode tools".to_string(),
        source_kind: "standalone_tools".to_string(),
        scope: ExternalSourceScope::UserGlobal,
        location: "/tools".to_string(),
        execution_domain_id: ExecutionDomainId::new("local-user").unwrap(),
        health: openbitfun_product_domains::external_sources::ExternalSourceHealth::Available,
        content_version: "v1".to_string(),
        diagnostics: Vec::new(),
    };
    let approval_key = "approval";
    let mut config = ExternalSourcesConfig {
        approved_tool_targets: BTreeSet::from([approval_key.to_string()]),
        ..ExternalSourcesConfig::default()
    };
    config
        .integration_policy
        .known_mut()
        .expect("the built-in policy schema is known")
        .user_defaults
        .enabled = true;

    config.suppressed_source_keys.push(source.preference_key());
    assert!(!external_tool_invocation_is_authorized_by(
        &config,
        source.ecosystem_id.as_str(),
        approval_key,
        &source.preference_key(),
        "<global>",
    ));
    assert!(external_tool_invocation_is_authorized_by(
        &config,
        source.ecosystem_id.as_str(),
        approval_key,
        &source.key.stable_key(),
        "<global>",
    ));
    config
        .integration_policy
        .known_mut()
        .expect("the built-in policy schema is known")
        .user_defaults
        .enabled = false;
    assert!(!external_tool_invocation_is_authorized_by(
        &config,
        source.ecosystem_id.as_str(),
        approval_key,
        &source.key.stable_key(),
        "<global>",
    ));
}
