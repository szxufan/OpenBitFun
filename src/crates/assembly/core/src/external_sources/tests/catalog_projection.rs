use super::super::*;
use super::support::test_service;
use openbitfun_product_domains::external_sources::{
    EcosystemId, ExternalSourceRecord, ExternalSourceScope,
};
use openbitfun_product_domains::workspace_references::ExternalWorkspaceReferenceDefinition;

#[test]
fn runtime_reason_projection_preserves_known_codes_and_hides_raw_errors() {
    for reason in [
        "external_mcp.start.authentication",
        "external_mcp.start.timeout",
        "external_mcp.runtime.host_unavailable",
        "external_mcp.runtime.server_missing",
        "external_mcp.runtime.failed",
        "external_mcp.runtime.stopped",
    ] {
        assert_eq!(stable_external_mcp_runtime_reason(reason), reason);
    }
    assert_eq!(
        stable_external_mcp_runtime_reason("failed to read D:/private/mcp.json: access denied"),
        EXTERNAL_MCP_RUNTIME_FAILED
    );
}

#[test]
fn effective_workspace_references_keep_native_order_before_external_aliases() {
    let native = vec![crate::service::workspace::RelatedPath {
        path: "D:/native-docs".to_string(),
        description: Some("OpenBitFun workspace setting".to_string()),
    }];
    let source_key = SourceKey::new("opencode.references", "project-config").unwrap();
    let external = openbitfun_external_sources::ExternalWorkspaceReferenceCoordinatorSnapshot {
        generation: 7,
        discovery_pending: false,
        sources: vec![ExternalSourceCatalogEntry {
            stable_key: source_key.stable_key(),
            presentation_group_id: None,
            record: ExternalSourceRecord {
                key: source_key.clone(),
                ecosystem_id: EcosystemId::new("opencode").unwrap(),
                display_name: "OpenCode project references".to_string(),
                source_kind: "opencode_config".to_string(),
                scope: ExternalSourceScope::Project,
                location: "D:/workspace/opencode.json".to_string(),
                execution_domain_id: ExecutionDomainId::new("local-user").unwrap(),
                health:
                    openbitfun_product_domains::external_sources::ExternalSourceHealth::Available,
                content_version: "source-v1".to_string(),
                diagnostics: Vec::new(),
            },
            lifecycle: ExternalSourceLifecycleState::Available,
        }],
        references: vec![ExternalWorkspaceReferenceDefinition {
            source: source_key,
            alias: "docs".to_string(),
            path: PathBuf::from("D:/external-docs"),
            description: Some("OpenCode reference".to_string()),
            hidden: false,
            content_version: "reference-v1".to_string(),
        }],
        diagnostics: Vec::new(),
    };

    let inactive =
        compose_workspace_reference_snapshot(&native, external.clone(), &BTreeSet::new());
    assert_eq!(inactive.references.len(), 1);
    assert_eq!(
        inactive.references[0].origin,
        WorkspaceReferenceOrigin::Native
    );

    let active_ecosystems = [EcosystemId::new("opencode").unwrap()]
        .into_iter()
        .collect();
    let snapshot = compose_workspace_reference_snapshot(&native, external, &active_ecosystems);

    assert_eq!(snapshot.generation, 7);
    assert_eq!(snapshot.references.len(), 2);
    assert_eq!(
        snapshot.references[0].origin,
        WorkspaceReferenceOrigin::Native
    );
    assert_eq!(snapshot.references[0].alias, None);
    assert_eq!(
        snapshot.references[1].origin,
        WorkspaceReferenceOrigin::External
    );
    assert_eq!(snapshot.references[1].alias.as_deref(), Some("docs"));
}

#[test]
fn canonical_json_is_independent_of_object_insertion_order() {
    let first = serde_json::json!({"outer": {"b": 2, "a": 1}});
    let second: serde_json::Value = serde_json::from_str(r#"{"outer":{"a":1,"b":2}}"#).unwrap();
    let mut first_bytes = Vec::new();
    let mut second_bytes = Vec::new();

    write_canonical_json(&first, &mut first_bytes).unwrap();
    write_canonical_json(&second, &mut second_bytes).unwrap();

    assert_eq!(first_bytes, second_bytes);
    assert_eq!(
        String::from_utf8(first_bytes).unwrap(),
        r#"{"outer":{"a":1,"b":2}}"#
    );
}

#[test]
fn external_sources_config_debug_redacts_the_persisted_revision_secret() {
    let config = ExternalSourcesConfig {
        mcp_revision_secret: Some("private-revision-secret".to_string()),
        ..Default::default()
    };

    let debug = format!("{config:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("private-revision-secret"));
}

#[test]
fn integration_error_metrics_decode_typed_codes_without_parsing_prose() {
    let stale = stale_operation_error("preferences changed");
    assert_eq!(external_integration_error_code(&stale), "stale_revision");
    assert_eq!(
        external_integration_error_code("legacy internal failure: private detail"),
        "internal"
    );
}

#[test]
fn background_log_categories_never_include_error_details_or_paths() {
    let stale = stale_operation_error("private workspace path changed");
    assert_eq!(external_log_error_category(&stale), "stale_revision");

    for raw in [
        r"directory_read_failed: C:\Users\alice\.config\opencode",
        "Failed to watch path /home/alice/.config/opencode: permission denied",
    ] {
        let category = external_log_error_category(raw);
        assert_eq!(category, "internal");
        assert!(!category.contains("alice"));
        assert!(!category.contains("opencode"));
    }
    assert_eq!(external_log_scope(Some(Path::new("C:/repo"))), "workspace");
    assert_eq!(external_log_scope::<&str>(None), "user-global");
}

#[test]
fn final_catalog_redacts_known_absolute_paths_from_diagnostics() {
    let source_key = SourceKey::new("future.tools", "project").unwrap();
    let raw_root = r"C:\Users\alice\repo\.future-ai\tools";
    let raw_file = format!(r"{raw_root}\review.js");
    let mut snapshot = ExternalSourceCatalogSnapshot {
        generation: 1,
        discovery_pending: false,
        sources: vec![ExternalSourceCatalogEntry {
            stable_key: source_key.stable_key(),
            presentation_group_id: None,
            record: ExternalSourceRecord {
                key: source_key.clone(),
                ecosystem_id: EcosystemId::new("future-ai").unwrap(),
                display_name: "Future AI project tools".to_string(),
                source_kind: "standalone_tools".to_string(),
                scope: ExternalSourceScope::Project,
                location: raw_root.to_string(),
                execution_domain_id: ExecutionDomainId::new("local-user").unwrap(),
                health: openbitfun_product_domains::external_sources::ExternalSourceHealth::Partial,
                content_version: "source-v1".to_string(),
                diagnostics: vec![ExternalSourceDiagnostic::warning(
                    "future.tool.directory_read_failed",
                    format!("Failed to read '{raw_root}'"),
                    Some(source_key.clone()),
                )
                .with_asset_kind(ExternalSourceAssetKind::Tool)],
            },
            lifecycle: ExternalSourceLifecycleState::Degraded,
        }],
        commands: Vec::new(),
        command_conflicts: Vec::new(),
        tools: Vec::new(),
        tool_approval_requests: Vec::new(),
        tool_conflicts: Vec::new(),
        mcp_generation: 0,
        mcp_servers: Vec::new(),
        mcp_approval_requests: Vec::new(),
        mcp_conflicts: Vec::new(),
        subagent_generation: 0,
        preference_revision: 0,
        subagents: Vec::new(),
        subagent_conflicts: Vec::new(),
        pending_subagent_approvals: Vec::new(),
        subagent_model_binding_groups: Vec::new(),
        subagent_model_binding_options: Vec::new(),
        integration_policy: Default::default(),
        diagnostics: vec![ExternalSourceDiagnostic::warning(
            "future.tool.file_read_failed",
            format!("Failed to read '{raw_file}'"),
            Some(source_key),
        )
        .with_asset_kind(ExternalSourceAssetKind::Tool)],
    };

    sanitize_external_snapshot_locations(&mut snapshot, Some(Path::new(r"C:\Users\alice\repo")));

    assert_eq!(
        snapshot.sources[0].record.location,
        "<workspace>/.future-ai/tools"
    );
    assert_eq!(
        snapshot.sources[0].record.diagnostics[0].message,
        "Failed to read '<workspace>/.future-ai/tools'"
    );
    assert_eq!(
        snapshot.diagnostics[0].message,
        "Failed to read '<workspace>/.future-ai/tools/review.js'"
    );
    let serialized = serde_json::to_string(&snapshot).unwrap();
    assert!(!serialized.contains("C:\\\\Users\\\\alice"));
    assert!(!serialized.contains("C:/Users/alice"));
}

#[test]
fn presentation_groups_are_assigned_before_location_redaction() {
    let make_source = |provider_id: &str, stable_key: &str, location: &str| {
        let source_key = SourceKey::new(provider_id, "user-configuration").unwrap();
        ExternalSourceCatalogEntry {
            stable_key: stable_key.to_string(),
            presentation_group_id: None,
            record: ExternalSourceRecord {
                key: source_key,
                ecosystem_id: EcosystemId::new("opencode").unwrap(),
                display_name: "OpenCode user configuration".to_string(),
                source_kind: "configuration".to_string(),
                scope: ExternalSourceScope::RemoteUser,
                location: location.to_string(),
                execution_domain_id: ExecutionDomainId::new("peer-a").unwrap(),
                health:
                    openbitfun_product_domains::external_sources::ExternalSourceHealth::Available,
                content_version: "source-v1".to_string(),
                diagnostics: Vec::new(),
            },
            lifecycle: ExternalSourceLifecycleState::Available,
        }
    };
    let mut snapshot = ExternalSourceCatalogSnapshot {
        generation: 0,
        discovery_pending: false,
        sources: vec![
            make_source(
                "opencode.commands",
                "command-source",
                "/remote/alice/.config/opencode/opencode.json",
            ),
            make_source(
                "opencode.subagents",
                "agent-source",
                "/remote/alice/.config/opencode/opencode.json",
            ),
            make_source(
                "opencode.mcp",
                "other-user-source",
                "/remote/bob/.config/opencode/opencode.json",
            ),
        ],
        commands: Vec::new(),
        command_conflicts: Vec::new(),
        tools: Vec::new(),
        tool_approval_requests: Vec::new(),
        tool_conflicts: Vec::new(),
        mcp_generation: 0,
        mcp_servers: Vec::new(),
        mcp_approval_requests: Vec::new(),
        mcp_conflicts: Vec::new(),
        subagent_generation: 0,
        preference_revision: 0,
        subagents: Vec::new(),
        subagent_conflicts: Vec::new(),
        pending_subagent_approvals: Vec::new(),
        subagent_model_binding_groups: Vec::new(),
        subagent_model_binding_options: Vec::new(),
        integration_policy: Default::default(),
        diagnostics: Vec::new(),
    };

    assign_external_source_presentation_groups(&mut snapshot);
    sanitize_external_snapshot_locations(&mut snapshot, None);

    assert_eq!(
        snapshot.sources[0].presentation_group_id,
        snapshot.sources[1].presentation_group_id,
    );
    assert_ne!(
        snapshot.sources[0].presentation_group_id,
        snapshot.sources[2].presentation_group_id,
    );
    assert!(snapshot
        .sources
        .iter()
        .all(|source| source.record.location == "<remote>/.config/opencode/opencode.json"));
}

#[test]
fn reveal_location_uses_stable_identity_and_keeps_the_raw_path_private() {
    let raw_location = std::env::current_dir()
        .unwrap()
        .join(".opencode")
        .join("opencode.json");
    let source = ExternalSourceCatalogEntry {
        stable_key: "opencode-project".to_string(),
        presentation_group_id: Some("opencode-project".to_string()),
        record: ExternalSourceRecord {
            key: SourceKey::new("opencode.commands", "project").unwrap(),
            ecosystem_id: EcosystemId::new("opencode").unwrap(),
            display_name: "OpenCode project configuration".to_string(),
            source_kind: "configuration".to_string(),
            scope: ExternalSourceScope::Project,
            location: raw_location.to_string_lossy().into_owned(),
            execution_domain_id: ExecutionDomainId::new("local-user").unwrap(),
            health: openbitfun_product_domains::external_sources::ExternalSourceHealth::Available,
            content_version: "source-v1".to_string(),
            diagnostics: Vec::new(),
        },
        lifecycle: ExternalSourceLifecycleState::Available,
    };

    let resolved =
        resolve_external_source_location("opencode-project", std::iter::once(&source)).unwrap();

    assert_eq!(resolved, raw_location);
    assert!(
        resolve_external_source_location("<workspace>/.opencode", std::iter::once(&source))
            .is_err()
    );
}

#[test]
fn surface_snapshot_projects_control_and_legacy_catalog_from_one_generation() {
    let service = test_service(Vec::new());

    let surface = service.surface_snapshot(ExternalSourceHostCapabilities::read_write());

    assert_eq!(
        surface.control.refresh_generation,
        surface.catalog.generation
    );
    assert_eq!(surface.control.capabilities.len(), 4);
    assert!(!surface.control.safe_mode);
}
