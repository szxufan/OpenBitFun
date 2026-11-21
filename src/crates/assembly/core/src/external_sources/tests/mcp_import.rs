use super::super::*;
use super::support::{blocked_provider, test_service, CountingExternalMcpRuntime};
use crate::service::mcp::{ConfigLocation, MCPServerConfig, MCPServerType};
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn imported_mcp_plan_does_not_treat_capacity_failures_as_an_empty_catalog() {
    let mut snapshot = openbitfun_external_sources::ExternalMcpCoordinatorSnapshot {
        generation: 1,
        discovery_pending: false,
        sources: vec![],
        servers: vec![],
        diagnostics: vec![],
    };
    assert!(mcp_import_discovery_complete(&snapshot));
    snapshot.discovery_pending = true;
    assert!(!mcp_import_discovery_complete(&snapshot));
    snapshot.discovery_pending = false;
    snapshot.diagnostics.push(ExternalSourceDiagnostic::warning(
        "external_mcp.discovery_overloaded",
        "Discovery capacity is busy",
        None,
    ));
    assert!(!mcp_import_discovery_complete(&snapshot));
    snapshot.diagnostics.clear();
    assert!(mcp_import_discovery_complete(&snapshot));
}

#[test]
fn imported_mcp_legacy_receipt_keeps_registered_origin_without_discovery() {
    use openbitfun_product_domains::external_sources::SourceQualifiedMcpServerId;
    for (provider, ecosystem) in [
        ("codex.mcp", "codex"),
        ("claude-code.mcp", "claude-code"),
        ("opencode.mcp", "opencode"),
    ] {
        let id = SourceQualifiedMcpServerId::new(
            SourceKey::new(provider, "removed-source").unwrap(),
            "old-server",
        )
        .unwrap();
        let candidate = format!("external_mcp:{}", id.stable_key());
        assert_eq!(
            ecosystem_for_imported_mcp_candidate(&candidate).as_deref(),
            Some(ecosystem)
        );
        assert!(
            ecosystem_for_imported_mcp_candidate(&format!("{candidate}invalid-tail")).is_none()
        );
    }
    assert!(ecosystem_for_imported_mcp_candidate("external_mcp:bad").is_none());
}

fn native_mcp_config_with_pin(pin: &str) -> MCPServerConfig {
    MCPServerConfig {
        id: "native-secret-test".to_string(),
        name: "native-secret-test".to_string(),
        server_type: MCPServerType::Local,
        transport: None,
        command: Some("native-secret-test".to_string()),
        args: Vec::new(),
        env: [("PIN".to_string(), pin.to_string())].into_iter().collect(),
        working_directory: None,
        inherit_parent_environment: Some(false),
        headers: Default::default(),
        url: None,
        auto_start: false,
        enabled: true,
        location: ConfigLocation::User,
        capabilities: Vec::new(),
        settings: Default::default(),
        oauth: None,
        oauth_enabled: None,
        xaa: None,
        timeouts: Default::default(),
    }
}

#[test]
fn native_mcp_behavior_versions_are_keyed_before_public_projection() {
    let key = ExternalMcpRevisionKey::new([7; 32]);
    let first_config = native_mcp_config_with_pin("0007");
    let changed_config = native_mcp_config_with_pin("0008");
    let first = native_mcp_behavior_version(&key, &first_config).unwrap();
    let repeated = native_mcp_behavior_version(&key, &first_config).unwrap();
    let changed = native_mcp_behavior_version(&key, &changed_config).unwrap();
    let raw_candidate = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&first_config).unwrap()))
    );

    assert_eq!(first, repeated);
    assert_ne!(first, changed);
    assert_ne!(first, raw_candidate);
    assert!(first.starts_with("hmac-sha256:"));
}

#[tokio::test]
async fn mcp_import_refresh_discovers_only_mcp_and_preserves_source_suppression() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("opencode.json"),
        r#"{"mcp":{"docs":{"type":"local","command":["docs"],"enabled":false}}}"#,
    )
    .unwrap();
    let unrelated_calls = Arc::new(AtomicUsize::new(0));
    let (unrelated, _release) = blocked_provider("opencode.commands", unrelated_calls.clone());
    let mut service = test_service(Vec::new());
    let runtime = Arc::new(CountingExternalMcpRuntime::default());
    let inner = Arc::get_mut(&mut service).unwrap();
    inner.profile = ExternalSourceServiceProfile::ReadOnlyProjection;
    inner.mcp_runtime = runtime.clone();
    inner.control_plane = Arc::new(
        ExternalSourceControlPlane::new(
            ExternalSourceContext {
                workspace_root: None,
                execution_domain_id: ExecutionDomainId::new("local-user").unwrap(),
            },
            ExternalMcpRevisionKey::new([7; 32]),
            vec![unrelated],
            Vec::new(),
            Vec::new(),
            vec![Arc::new(OpenCodeMcpProvider::new(
                OpenCodeMcpProviderOptions {
                    config: OpenCodeCommandProviderOptions {
                        user_config_dir: directory.path().to_path_buf(),
                        legacy_user_config_dir: None,
                        explicit_config_file: None,
                        explicit_config_dir: None,
                        inline_config_content: None,
                        project_config_enabled: false,
                    },
                    project_root_override: None,
                },
            ))],
            Vec::new(),
        )
        .unwrap(),
    );
    let mut preferences = ExternalSourcesConfig::default();
    let disabled_policy = integration_policy_snapshot(&preferences, None).unwrap();
    service.discover_mcp_import_sources(&disabled_policy).await;
    assert!(lock_mcp_coordinator(&service.control_plane)
        .snapshot()
        .servers
        .is_empty());
    preferences
        .integration_policy
        .known_mut()
        .unwrap()
        .user_defaults
        .enabled = true;
    let policy = integration_policy_snapshot(&preferences, None).unwrap();
    service.discover_mcp_import_sources(&policy).await;
    let snapshot = lock_mcp_coordinator(&service.control_plane).snapshot();
    assert_eq!(snapshot.servers.len(), 1);
    assert_eq!(unrelated_calls.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 0);
    let source_key = snapshot.sources[0].stable_key.clone();
    lock_mcp_coordinator(&service.control_plane)
        .set_source_enabled(&source_key, false)
        .unwrap();
    service.discover_mcp_import_sources(&policy).await;
    assert!(lock_mcp_coordinator(&service.control_plane)
        .snapshot()
        .servers
        .is_empty());
}

#[tokio::test]
async fn read_only_projection_never_calls_the_mcp_runtime() {
    let runtime = Arc::new(CountingExternalMcpRuntime::default());
    let mut service = test_service(Vec::new());
    let service_inner = Arc::get_mut(&mut service).expect("test owns the service");
    service_inner.profile = ExternalSourceServiceProfile::ReadOnlyProjection;
    service_inner.mcp_runtime = runtime.clone();

    let preferences = ExternalSourcesConfig::default();
    let policy = integration_policy_snapshot(&preferences, None).unwrap();
    let command_snapshot = lock_coordinator(&service.control_plane).snapshot();
    let snapshot = service
        .rebuild_read_only_projection(command_snapshot, preferences, policy)
        .await
        .unwrap();

    assert_eq!(runtime.calls.load(Ordering::SeqCst), 0);
    assert!(snapshot
        .mcp_servers
        .iter()
        .all(|entry| entry.runtime_id.is_none()));
    assert!(snapshot
        .tools
        .iter()
        .all(|entry| { !matches!(entry.activation, ExternalToolActivationState::Active) }));
    assert!(snapshot.subagents.iter().all(|entry| {
        !matches!(
            entry.activation_state,
            ExternalSubagentActivationState::Active
        )
    }));
}

#[test]
fn mcp_server_decisions_keep_one_version_per_workspace_lineage() {
    let mut decisions = BTreeMap::from([
        (
            "external_mcp_approval:local-user:workspace-a:server:old".to_string(),
            ExternalMcpDecision {
                decision_key: "external_mcp_approval:local-user:workspace-a:server:old".to_string(),
                approved: true,
            },
        ),
        (
            "external_mcp_approval:local-user:workspace-b:server:current".to_string(),
            ExternalMcpDecision {
                decision_key: "external_mcp_approval:local-user:workspace-b:server:current"
                    .to_string(),
                approved: true,
            },
        ),
    ]);

    reconcile_versioned_mcp_server_decision(
        &mut decisions,
        "external_mcp_approval:local-user:workspace-a:server:new".to_string(),
        false,
    );

    assert!(!decisions.contains_key("external_mcp_approval:local-user:workspace-a:server:old"));
    assert!(!decisions["external_mcp_approval:local-user:workspace-a:server:new"].approved);
    assert!(decisions.contains_key("external_mcp_approval:local-user:workspace-b:server:current"));
    assert_eq!(decisions.len(), 2);
}
