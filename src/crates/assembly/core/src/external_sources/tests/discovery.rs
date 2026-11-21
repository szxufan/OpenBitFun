use super::super::*;
use super::support::{
    blocked_provider, delayed_provider, test_service, CountingExternalMcpRuntime,
};
use openbitfun_product_domains::external_sources::{EcosystemId, ExternalSourceScope};
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn automatic_discovery_mutations_preserve_runtime_policy_approvals_and_scope() {
    let mut config = ExternalSourcesConfig::default();
    config.approved_tool_targets.insert("approved-tool".into());
    config
        .approved_subagent_envelopes
        .insert("approved-agent".into());
    for runtime_enabled in [false, true] {
        config
            .integration_policy
            .known_mut()
            .unwrap()
            .user_defaults
            .enabled = runtime_enabled;
        let before = integration_policy_snapshot(&config, None).unwrap();
        for enabled in [true, false] {
            let mutation = ExternalIntegrationPolicyMutation {
                expected_preference_revision: config.preference_revision,
                scope: ExternalIntegrationPolicyScope::Workspace,
                change: ExternalIntegrationPolicyOperation::SetAutomaticDiscovery { enabled },
            };
            apply_integration_policy_mutation_to_config(&mut config, Some("project"), &mutation)
                .unwrap();
            assert_eq!(
                integration_policy_snapshot(&config, None)
                    .unwrap()
                    .effective,
                before.effective
            );
            let document = config.integration_policy.known().unwrap();
            assert_eq!(
                automatic_discovery_enabled(document, Some("project")),
                enabled
            );
            assert_eq!(
                automatic_discovery_enabled(document, Some("another-project")),
                runtime_enabled
            );
            assert!(config.approved_tool_targets.contains("approved-tool"));
            assert!(config
                .approved_subagent_envelopes
                .contains("approved-agent"));
            let revision = config.preference_revision;
            assert!(apply_integration_policy_mutation_to_config(
                &mut config,
                Some("project"),
                &mutation
            )
            .is_err());
            assert_eq!(config.preference_revision, revision);
        }
    }
}

#[test]
fn automatic_discovery_policy_only_bypasses_the_runtime_master_switch() {
    let mut service = test_service(Vec::new());
    Arc::get_mut(&mut service).unwrap().profile = ExternalSourceServiceProfile::DiscoveryCatalog;
    let mut preferences = ExternalSourcesConfig::default();
    preferences
        .integration_policy
        .known_mut()
        .unwrap()
        .user_defaults
        .ecosystems
        .insert(
            EcosystemId::new("codex").unwrap(),
            openbitfun_product_domains::external_integration_policy::ExternalEcosystemPolicy {
                mode: ExternalIntegrationMode::Disabled,
                ..Default::default()
            },
        );
    let original = preferences.clone();
    let discovery = service.discovery_policy(&preferences).unwrap();
    assert!(integration_capability_is_discoverable(
        &discovery,
        "opencode",
        EXTERNAL_CAPABILITY_MCP
    ));
    assert!(!integration_capability_is_discoverable(
        &discovery,
        "codex",
        EXTERNAL_CAPABILITY_MCP
    ));
    assert!(
        !integration_policy_snapshot(&preferences, None)
            .unwrap()
            .effective
            .enabled
    );
    assert_eq!(
        serde_json::to_value(preferences).unwrap(),
        serde_json::to_value(original).unwrap()
    );
}

#[tokio::test]
async fn automatic_discovery_catalog_discovers_without_calling_or_retiring_runtime() {
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = Arc::new(CountingExternalMcpRuntime::default());
    let mut service = test_service(vec![delayed_provider(
        "opencode",
        std::time::Duration::ZERO,
        calls.clone(),
    )]);
    let inner = Arc::get_mut(&mut service).unwrap();
    inner.profile = ExternalSourceServiceProfile::DiscoveryCatalog;
    inner.mcp_runtime = runtime.clone();
    service
        .active_mcp_runtime_ids
        .lock()
        .await
        .insert("existing-runtime".into());
    let catalog = service.refresh().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(catalog.commands.len(), 1);
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 0);
    assert!(service
        .active_mcp_runtime_ids
        .lock()
        .await
        .contains("existing-runtime"));
    assert!(service.initial_refresh_completed.load(Ordering::Acquire));
}

async fn refresh_test_commands(
    service: &Arc<WorkspaceExternalSourceService>,
) -> ExternalSourceCatalogSnapshot {
    let requests = lock_coordinator(&service.control_plane).discovery_requests();
    let batch = service
        .control_plane
        .discover_commands(requests, std::time::Duration::from_millis(25))
        .await;
    let snapshot =
        lock_coordinator(&service.control_plane).apply_discovery_results(batch.immediate);
    for deferred in batch.deferred {
        let control_plane = Arc::clone(&service.control_plane);
        tokio::spawn(async move {
            let Some((completed, _observer)) = control_plane.complete_command(deferred).await
            else {
                return;
            };
            let Some(result) = control_plane.finalize_command(completed).await else {
                return;
            };
            lock_coordinator(&control_plane).apply_discovery_result(result);
        });
    }
    snapshot
}

#[test]
fn composition_projects_opencode_configured_skill_roots_without_leaking_adapter_types() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let user_config = home.join(".config/opencode");
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::create_dir_all(project.join("shared-skills")).unwrap();
    std::fs::create_dir_all(&user_config).unwrap();
    std::fs::write(
        project.join("opencode.json"),
        r#"{"skills":["shared-skills"]}"#,
    )
    .unwrap();
    let provider = OpenCodeSkillRootProvider::new(OpenCodeSkillRootProviderOptions {
        config: OpenCodeCommandProviderOptions {
            user_config_dir: user_config,
            legacy_user_config_dir: Some(home.join(".opencode")),
            explicit_config_file: None,
            explicit_config_dir: None,
            inline_config_content: None,
            project_config_enabled: true,
        },
        home_dir: Some(home),
    });

    let (roots, diagnostics) =
        opencode_configured_skill_roots_with_provider(Some(&project), &provider);
    assert!(diagnostics.is_empty());

    assert_eq!(roots.len(), 1);
    assert_eq!(
        roots[0].path,
        dunce::canonicalize(project.join("shared-skills")).unwrap()
    );
    assert_eq!(roots[0].scope, ExternalSourceScope::Project);
}

#[tokio::test]
async fn slow_provider_is_not_respawned_while_healthy_sibling_updates() {
    let slow_calls = Arc::new(AtomicUsize::new(0));
    let healthy_calls = Arc::new(AtomicUsize::new(0));
    let (slow_provider, release_slow_provider) = blocked_provider("slow", Arc::clone(&slow_calls));
    let service = test_service(vec![
        slow_provider,
        delayed_provider(
            "healthy",
            std::time::Duration::from_millis(50),
            Arc::clone(&healthy_calls),
        ),
    ]);

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let _ = refresh_test_commands(&service).await;
            if slow_calls.load(Ordering::SeqCst) == 1 && healthy_calls.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("slow and healthy providers must both start");

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let snapshot = lock_coordinator(&service.control_plane).snapshot();
            if snapshot
                .commands
                .iter()
                .any(|command| command.definition.name == "healthy")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the healthy provider result must be published");

    let healthy_calls_before_refresh = healthy_calls.load(Ordering::SeqCst);
    let snapshot = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let snapshot = refresh_test_commands(&service).await;
            if healthy_calls.load(Ordering::SeqCst) > healthy_calls_before_refresh {
                break snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("healthy provider must refresh while the slow provider is blocked");

    assert_eq!(slow_calls.load(Ordering::SeqCst), 1);
    assert!(snapshot
        .commands
        .iter()
        .any(|command| command.definition.name == "healthy"));
    let _ = release_slow_provider.send(());
}

#[tokio::test]
async fn initial_refresh_waiters_reuse_the_in_flight_result() {
    let service = test_service(Vec::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());

    let background = {
        let service = Arc::clone(&service);
        let snapshot_service = Arc::clone(&service);
        let calls = Arc::clone(&calls);
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        tokio::spawn(async move {
            service
                .ensure_initial_refresh_with(|| async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    started.notify_one();
                    release.notified().await;
                    Ok(snapshot_service.snapshot())
                })
                .await
        })
    };

    started.notified().await;
    let catalog_waiter = {
        let service = Arc::clone(&service);
        let snapshot_service = Arc::clone(&service);
        let calls = Arc::clone(&calls);
        tokio::spawn(async move {
            service
                .ensure_initial_refresh_with(|| async move {
                    calls.fetch_add(100, Ordering::SeqCst);
                    Ok(snapshot_service.snapshot())
                })
                .await
        })
    };
    tokio::task::yield_now().await;
    assert!(!catalog_waiter.is_finished());

    release.notify_one();
    background.await.unwrap().unwrap();
    catalog_waiter.await.unwrap().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_initial_refresh_can_be_retried() {
    let service = test_service(Vec::new());
    let first = service
        .ensure_initial_refresh_with(|| async { Err("temporary failure".to_string()) })
        .await;
    assert_eq!(first.unwrap_err(), "temporary failure");

    let calls = Arc::new(AtomicUsize::new(0));
    let snapshot_service = Arc::clone(&service);
    service
        .ensure_initial_refresh_with(|| {
            let calls = Arc::clone(&calls);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(snapshot_service.snapshot())
            }
        })
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
