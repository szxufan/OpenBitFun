use super::super::*;
use openbitfun_product_domains::external_sources::EcosystemId;

#[tokio::test]
async fn preference_store_merges_updates_from_independent_instances() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("external-sources.json");
    let first = ExternalSourcePreferenceStore::new(path.clone());
    let second = ExternalSourcePreferenceStore::new(path);

    let disable = first.update(|config| {
        config
            .suppressed_source_keys
            .push("opencode:global".to_string());
    });
    let choose = second.update(|config| {
        ExternalSourceCoordinator::reconcile_conflict_preferences(
            &mut config.conflict_choices,
            &mut config.conflict_lineage_current_keys,
            &mut config.conflicted_candidate_ids,
            "prompt_command:local-user:review:v1",
            "candidate-a",
            &["candidate-a".to_string(), "candidate-b".to_string()],
        );
    });
    let (disabled, chosen) = tokio::join!(disable, choose);
    disabled.unwrap();
    chosen.unwrap();

    let persisted = first.read().await.unwrap();
    assert_eq!(persisted.suppressed_source_keys, ["opencode:global"]);
    assert_eq!(
        persisted
            .conflict_choices
            .get("prompt_command:local-user:review:v1")
            .map(String::as_str),
        Some("candidate-a")
    );
    assert_eq!(
        persisted.conflict_lineage_current_keys["prompt_command:local-user:review"],
        "prompt_command:local-user:review:v1"
    );
    assert_eq!(
        persisted.conflicted_candidate_ids,
        BTreeSet::from(["candidate-a".to_string(), "candidate-b".to_string()])
    );
}

#[test]
fn preference_document_preserves_future_minor_fields() {
    let raw = serde_json::json!({
        "integrationPolicy": {
            "schemaMajor": 1,
            "userDefaults": {
                "enabled": true,
                "futureSetting": "keep"
            },
            "futurePolicyField": { "revision": 2 }
        },
        "preferenceRevision": 4,
        "futurePreferenceField": ["keep"]
    });
    let mut config: ExternalSourcesConfig = serde_json::from_value(raw).unwrap();
    config.preference_revision += 1;
    let encoded = serde_json::to_value(config).unwrap();

    assert_eq!(
        encoded["integrationPolicy"]["userDefaults"]["futureSetting"],
        "keep"
    );
    assert_eq!(
        encoded["integrationPolicy"]["futurePolicyField"]["revision"],
        2
    );
    assert_eq!(encoded["futurePreferenceField"][0], "keep");
}

fn retired_application_default_fixture() -> ExternalSourcesConfig {
    let mut config = ExternalSourcesConfig::default();
    let policy = config
        .integration_policy
        .known_mut()
        .expect("the built-in integration policy is known");
    policy.user_defaults.enabled = true;
    for (ecosystem, mode) in [
        (OPENCODE_ECOSYSTEM_ID, ExternalIntegrationMode::Recommended),
        (
            CLAUDE_CODE_ECOSYSTEM_ID,
            ExternalIntegrationMode::DiscoverOnly,
        ),
        (CODEX_ECOSYSTEM_ID, ExternalIntegrationMode::DiscoverOnly),
    ] {
        policy
            .user_defaults
            .ecosystems
            .entry(EcosystemId::new(ecosystem).unwrap())
            .or_default()
            .mode = mode;
    }
    config
}

fn retired_application_document(
    config: ExternalSourcesConfig,
    decisions: serde_json::Value,
) -> serde_json::Value {
    let mut raw = serde_json::to_value(config).unwrap();
    raw["configOrigin"] = serde_json::json!("fresh_v2");
    raw["connectionSchemaMigrationVersion"] = serde_json::json!(1);
    raw["applicationConnections"] = decisions;
    raw
}

#[tokio::test]
async fn retired_automatic_application_default_is_not_user_consent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("external-sources.json");
    let raw =
        retired_application_document(retired_application_default_fixture(), serde_json::json!({}));
    std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    let store = ExternalSourcePreferenceStore::new(path);

    let read = store.read().await.unwrap();
    let read_policy = read.integration_policy.known().unwrap();
    assert!(!read_policy.user_defaults.enabled);
    assert!(read_policy.user_defaults.ecosystems.is_empty());
    assert!(read.extensions.contains_key("applicationConnections"));

    let (was_enabled, updated) = store
        .update(|config| {
            config
                .integration_policy
                .known()
                .unwrap()
                .user_defaults
                .enabled
        })
        .await
        .unwrap();
    assert!(!was_enabled);
    assert!(
        !updated
            .integration_policy
            .known()
            .unwrap()
            .user_defaults
            .enabled
    );
}

#[tokio::test]
async fn retired_migration_is_consumed_before_later_user_policy_changes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("external-sources.json");
    let mut config = retired_application_default_fixture();
    config
        .integration_policy
        .known_mut()
        .unwrap()
        .user_defaults
        .ecosystems
        .get_mut(&EcosystemId::new(CLAUDE_CODE_ECOSYSTEM_ID).unwrap())
        .unwrap()
        .mode = ExternalIntegrationMode::Disabled;
    let raw = retired_application_document(config, serde_json::json!({}));
    std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    let store = ExternalSourcePreferenceStore::new(path);

    let (_, migrated) = store.update(|_| {}).await.unwrap();
    assert!(!migrated.extensions.contains_key("configOrigin"));
    assert_eq!(
        migrated
            .integration_policy
            .known()
            .unwrap()
            .user_defaults
            .ecosystems[&EcosystemId::new(CLAUDE_CODE_ECOSYSTEM_ID).unwrap()]
            .mode,
        ExternalIntegrationMode::Disabled
    );

    store
        .update(|config| {
            config.integration_policy =
                StoredExternalIntegrationPolicy::Known(retired_automatic_application_policy());
        })
        .await
        .unwrap();

    let read = store.read().await.unwrap();
    assert_eq!(
        read.integration_policy.known(),
        Some(&retired_automatic_application_policy())
    );
}

#[tokio::test]
async fn retired_application_metadata_preserves_a_policy_user_deviation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("external-sources.json");
    let mut config = retired_application_default_fixture();
    config
        .integration_policy
        .known_mut()
        .unwrap()
        .user_defaults
        .ecosystems
        .get_mut(&EcosystemId::new(CLAUDE_CODE_ECOSYSTEM_ID).unwrap())
        .unwrap()
        .mode = ExternalIntegrationMode::Disabled;
    let raw = retired_application_document(config, serde_json::json!({}));
    std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();

    let config = ExternalSourcePreferenceStore::new(path)
        .read()
        .await
        .unwrap();
    let policy = config.integration_policy.known().unwrap();

    assert!(policy.user_defaults.enabled);
    assert_eq!(
        policy.user_defaults.ecosystems[&EcosystemId::new(CLAUDE_CODE_ECOSYSTEM_ID).unwrap()].mode,
        ExternalIntegrationMode::Disabled
    );
}

#[tokio::test]
async fn retired_application_metadata_preserves_an_explicit_application_choice() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("external-sources.json");
    let raw = retired_application_document(
        retired_application_default_fixture(),
        serde_json::json!({
            "local-user\u{1f}opencode\u{1f}user_default": {
                "desiredConnection": "connected",
                "decisionOrigin": "user"
            }
        }),
    );
    std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();

    let config = ExternalSourcePreferenceStore::new(path)
        .read()
        .await
        .unwrap();

    assert!(
        config
            .integration_policy
            .known()
            .unwrap()
            .user_defaults
            .enabled
    );
    assert!(config.extensions.contains_key("applicationConnections"));
}

#[test]
fn incompatible_policy_requires_explicit_reset_and_keeps_a_bounded_backup() {
    let future_policy = serde_json::json!({
        "schemaMajor": 13,
        "userDefaults": "future-policy-shape",
        "workspaceOverrides": ["also", "structurally", "different"],
        "futurePolicyField": { "schema": 13 }
    });
    let stored_future_policy: StoredExternalIntegrationPolicy =
        serde_json::from_value(future_policy.clone()).unwrap();
    let mut config = ExternalSourcesConfig {
        integration_policy: stored_future_policy,
        integration_policy_backups: vec![
            serde_json::json!({ "schemaMajor": 10, "opaque": "first" }),
            serde_json::json!({ "schemaMajor": 11, "opaque": "second" }),
            serde_json::json!({ "schemaMajor": 12, "opaque": "third" }),
        ],
        preference_revision: 7,
        ..ExternalSourcesConfig::default()
    };
    let public_snapshot = integration_policy_snapshot(&config, None).unwrap();
    assert_eq!(
        public_snapshot.status,
        ExternalIntegrationPolicyStatus::IncompatibleSchema
    );
    assert!(!public_snapshot.global_effective.enabled);
    assert!(!public_snapshot.effective.enabled);
    let serialized_snapshot = serde_json::to_value(&public_snapshot).unwrap();
    assert!(!serialized_snapshot
        .to_string()
        .contains("future-policy-shape"));
    assert!(!serialized_snapshot
        .to_string()
        .contains("futurePolicyField"));

    config
        .suppressed_source_keys
        .push("opencode:project".to_string());
    let persisted = serde_json::to_value(&config).unwrap();
    config = serde_json::from_value(persisted).unwrap();
    assert_eq!(config.integration_policy.raw_value(), future_policy);
    assert_eq!(config.suppressed_source_keys, ["opencode:project"]);

    let ordinary_mutation = ExternalIntegrationPolicyMutation {
        expected_preference_revision: 7,
        scope: ExternalIntegrationPolicyScope::User,
        change: ExternalIntegrationPolicyOperation::SetEnabled { enabled: false },
    };
    let error = apply_integration_policy_mutation_to_config(&mut config, None, &ordinary_mutation)
        .expect_err("future schemas cannot be edited by an older host");
    assert_eq!(
        ExternalSourceOperationError::decode(&error)
            .expect("incompatible schemas use the typed error contract")
            .code,
        ExternalSourceOperationErrorCode::PolicyIncompatible
    );
    assert_eq!(config.preference_revision, 7);
    assert_eq!(config.integration_policy.schema_major(), 13);

    let reset = ExternalIntegrationPolicyMutation {
        expected_preference_revision: 7,
        scope: ExternalIntegrationPolicyScope::User,
        change: ExternalIntegrationPolicyOperation::ResetIncompatiblePolicy,
    };
    assert!(apply_integration_policy_mutation_to_config(&mut config, None, &reset).unwrap());
    assert_eq!(config.preference_revision, 8);
    assert_eq!(
        config.integration_policy.schema_major(),
        EXTERNAL_INTEGRATION_POLICY_SCHEMA_MAJOR
    );
    assert!(
        !integration_policy_snapshot(&config, None)
            .unwrap()
            .effective
            .enabled
    );
    assert_eq!(
        config
            .integration_policy_backups
            .iter()
            .map(|document| document["schemaMajor"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![11, 12, 13]
    );
    assert_eq!(config.integration_policy_backups[2], future_policy);
}

#[tokio::test]
async fn invalid_preference_file_is_an_error_instead_of_resetting_choices() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("external-sources.json");
    tokio::fs::write(&path, "{ invalid json").await.unwrap();

    let error = ExternalSourcePreferenceStore::new(path)
        .read()
        .await
        .expect_err("invalid preferences must fail closed");

    assert!(error.contains("deserialize"));
}
