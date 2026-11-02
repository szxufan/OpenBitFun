//! A single-document downgrade boundary for explicit subscription accounts.
//! Readers that do not understand account bindings retain raw provider/model
//! metadata but see an API-key projection which cannot reach a real provider
//! or obtain a provider-default subscription credential.
//!
//! Connection facts (auth, endpoint, credential, transport overrides) live on
//! the provider record; channel routing lives on model entries. The guard
//! therefore wraps guarded providers and channel-guarded entries separately:
//! provider guards use versions 1 (subscription with account id) and 2
//! (subscription pool), model entry guards use version 3 (channel routing).

use crate::{
    AIModelConfig, AIModelEntry, AIProviderConfig, AuthConfig, ConfigDiagnostic,
    ConfigDiagnosticRecoverability, ConfigDiagnosticSeverity,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub const ACCOUNT_MODEL_GUARD_KEY: &str = "openbitfun_subscription_account_binding";
pub const ACCOUNT_MODEL_GUARD_URL: &str = "https://subscription-accounts.openbitfun.invalid";
const GUARD_API_KEY: &str = "requires-subscription-accounts-v1";
const AUTH_BINDING_KEYS: &[&str] = &[
    "type",
    "provider",
    "plan",
    "account_id",
    "accounts",
    "revision",
];

fn id_counts_at(value: &Value, pointer: &str) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for id in value
        .pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|record| record.get("id").and_then(Value::as_str))
    {
        *counts.entry(id.to_string()).or_default() += 1;
    }
    counts
}

/// Opaque compatible-source fields survive new-version settings writes too.
/// The manager retains only these records, not a second config document.
/// Provider sources are keyed as `provider:<id>`, model entry sources as
/// `model:<id>`.
pub fn account_guard_sources(value: &Value) -> HashMap<String, Value> {
    let mut sources = HashMap::new();
    for (pointer, prefix) in [("/ai/providers", "provider:"), ("/ai/models", "model:")] {
        for record in value
            .pointer(pointer)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some((id, entry)) = record
                .get("id")
                .and_then(Value::as_str)
                .map(|id| id.to_string())
                .zip(guard_entry(record).cloned())
            {
                sources.insert(format!("{prefix}{id}"), entry);
            }
        }
    }
    sources
}

pub fn preserve_account_guard_sources(value: &mut Value, sources: &HashMap<String, Value>) {
    if sources.is_empty() {
        return;
    }
    for (pointer, prefix, known_fields) in [
        (
            "/ai/providers",
            "provider:",
            known_persisted_fields::<AIProviderConfig>(),
        ),
        (
            "/ai/models",
            "model:",
            known_persisted_fields::<AIModelEntry>(),
        ),
    ] {
        let id_counts = id_counts_at(value, pointer);
        let Some(records) = value.pointer_mut(pointer).and_then(Value::as_array_mut) else {
            continue;
        };
        for record in records.iter_mut() {
            let Some(id) = record.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(source) = sources.get(&format!("{prefix}{id}")) else {
                continue;
            };
            // Ambiguous IDs remain wrapped instead of being decoded, so each row
            // already retains its complete opaque record. Never merge a source
            // indexed by ID into another row with the same ID.
            if id_counts.get(id).is_some_and(|count| *count > 1) {
                continue;
            }
            let Some(current) = record
                .get_mut("metadata")
                .and_then(|metadata| metadata.get_mut(ACCOUNT_MODEL_GUARD_KEY))
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            if !matches!(
                current.get("version").and_then(Value::as_u64),
                Some(1 | 2 | 3)
            ) || current.get("version") != source.get("version")
            {
                continue;
            }
            let same_binding = AUTH_BINDING_KEYS.iter().all(|key| {
                let left = current
                    .get("source")
                    .and_then(|source| source.get("auth"))
                    .and_then(|auth| auth.get(key))
                    .filter(|value| !value.is_null());
                let right = source
                    .get("source")
                    .and_then(|source| source.get("auth"))
                    .and_then(|auth| auth.get(key))
                    .filter(|value| !value.is_null());
                if *key == "revision" {
                    left.and_then(Value::as_u64).unwrap_or(0)
                        == right.and_then(Value::as_u64).unwrap_or(0)
                } else if *key == "accounts" {
                    left.and_then(Value::as_array)
                        .map(Vec::as_slice)
                        .unwrap_or(&[])
                        == right
                            .and_then(Value::as_array)
                            .map(Vec::as_slice)
                            .unwrap_or(&[])
                } else {
                    left == right
                }
            });
            if !same_binding {
                continue;
            }
            if let Some(fields) = source.as_object() {
                for (key, data) in fields {
                    if key != "version" && key != "source" {
                        current.insert(key.clone(), data.clone());
                    }
                }
            }
            if let Some(original) = source.get("source").and_then(Value::as_object) {
                if let Some(updated) = current.get_mut("source").and_then(Value::as_object_mut) {
                    for (key, data) in original {
                        if !known_fields.contains(key) {
                            updated.insert(key.clone(), data.clone());
                        }
                    }
                    if let Some(auth) = updated.get_mut("auth").and_then(Value::as_object_mut) {
                        for (key, data) in original
                            .get("auth")
                            .and_then(Value::as_object)
                            .into_iter()
                            .flatten()
                        {
                            if !AUTH_BINDING_KEYS.contains(&key.as_str()) {
                                auth.insert(key.clone(), data.clone());
                            }
                        }
                    }
                    if pointer == "/ai/models" {
                        if let Some(route) = updated
                            .get_mut("channel_routing")
                            .and_then(Value::as_object_mut)
                        {
                            for (key, data) in original
                                .get("channel_routing")
                                .and_then(Value::as_object)
                                .into_iter()
                                .flatten()
                            {
                                if !["version", "fallback_model_ids", "manual_channel"]
                                    .contains(&key.as_str())
                                {
                                    route.entry(key.clone()).or_insert_with(|| data.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn known_persisted_fields<T: Default + serde::Serialize>() -> std::collections::HashSet<String> {
    let mut fields: std::collections::HashSet<String> = serde_json::to_value(T::default())
        .unwrap_or(Value::Null)
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    fields.insert("reasoning".into());
    fields.insert("channel_routing".into());
    fields
}

fn explicit_account_provider(provider: &Value) -> bool {
    // New subscription tags need the same downgrade boundary even when they
    // use their provider-default slot. `workbuddy`/`workbuddy_ai` are retired
    // providers, but the records an earlier release wrote under those tags
    // keep this boundary.
    provider.pointer("/auth/type").and_then(Value::as_str) == Some("subscription_pool")
        || (provider.pointer("/auth/type").and_then(Value::as_str) == Some("subscription")
            && (provider
                .pointer("/auth/account_id")
                .is_some_and(|id| !id.is_null())
                || matches!(
                    provider.pointer("/auth/provider").and_then(Value::as_str),
                    Some("workbuddy" | "workbuddy_ai" | "zcode" | "zcode_cn")
                )))
}

fn explicit_account_entry(entry: &Value) -> bool {
    // A channel routing may reference an account model; readers without
    // channel support must not follow its fallback chain.
    entry
        .get("channel_routing")
        .is_some_and(|route| !route.is_null())
}

fn guard_entry(record: &Value) -> Option<&Value> {
    record.get("metadata")?.get(ACCOUNT_MODEL_GUARD_KEY)
}

/// Reject these records at runtime if their payload cannot be restored. A
/// protected export must not accidentally become a working API-key model.
pub fn is_guarded_account_model(model: &AIModelConfig) -> bool {
    model.auth == AuthConfig::ApiKey
        && (model.api_key == GUARD_API_KEY || model.base_url == ACCOUNT_MODEL_GUARD_URL)
}

fn insert_guard_metadata(fields: &mut Map<String, Value>, entry: Value) {
    let metadata = fields
        .entry("metadata")
        .or_insert_with(|| Value::Object(Map::new()));
    if !metadata.is_object() {
        *metadata = Value::Object(Map::new());
    }
    metadata
        .as_object_mut()
        .unwrap()
        .insert(ACCOUNT_MODEL_GUARD_KEY.into(), entry);
}

fn make_provider_projection(provider: &mut Value, entry: Value) {
    let Some(fields) = provider.as_object_mut() else {
        return;
    };
    insert_guard_metadata(fields, entry);
    fields.insert("auth".into(), json!({"type":"api_key"}));
    fields.insert("api_key".into(), GUARD_API_KEY.into());
    fields.insert("base_url".into(), ACCOUNT_MODEL_GUARD_URL.into());
    fields.insert("custom_headers".into(), Value::Null);
    fields.insert("custom_request_body".into(), Value::Null);
}

fn make_entry_projection(entry_record: &mut Value, entry: Value) {
    let Some(fields) = entry_record.as_object_mut() else {
        return;
    };
    insert_guard_metadata(fields, entry);
    fields.remove("channel_routing");
}

fn guard_version(provider: &Value) -> u64 {
    if provider.pointer("/auth/type").and_then(Value::as_str) == Some("subscription_pool") {
        2
    } else {
        1
    }
}

/// Protects persisted/exported values without changing the live DTO or its
/// approved runtime binding fingerprint. No second file or vault mutation.
pub fn protect_account_models(value: &mut Value) {
    if let Some(providers) = value
        .pointer_mut("/ai/providers")
        .and_then(Value::as_array_mut)
    {
        for provider in providers.iter_mut() {
            if explicit_account_provider(provider) {
                let original = provider.clone();
                let version = guard_version(provider);
                make_provider_projection(provider, json!({"version":version,"source":original}));
            } else if let Some(entry) = guard_entry(provider).cloned() {
                // Unknown versions remain raw and unusable even after unrelated
                // settings writes. Never replace their retained source payload.
                if provider.get("api_key").and_then(Value::as_str) == Some(GUARD_API_KEY)
                    || provider.get("base_url").and_then(Value::as_str)
                        == Some(ACCOUNT_MODEL_GUARD_URL)
                {
                    make_provider_projection(provider, entry);
                }
            }
        }
    }
    if let Some(models) = value
        .pointer_mut("/ai/models")
        .and_then(Value::as_array_mut)
    {
        for model in models.iter_mut() {
            if explicit_account_entry(model) {
                let original = model.clone();
                make_entry_projection(model, json!({"version":3,"source":original}));
            } else if let Some(entry) = guard_entry(model).cloned() {
                if model.get("channel_routing").is_none() {
                    make_entry_projection(model, entry);
                }
            }
        }
    }
}

/// Restores only this codec's supported account records. Unsupported or
/// malformed source data stays in metadata behind the safe projection.
pub fn restore_account_models(value: &mut Value) -> Vec<ConfigDiagnostic> {
    let mut diagnostics = Vec::new();
    let provider_counts = id_counts_at(value, "/ai/providers");
    if let Some(providers) = value
        .pointer_mut("/ai/providers")
        .and_then(Value::as_array_mut)
    {
        for (index, provider) in providers.iter_mut().enumerate() {
            let Some(entry) = guard_entry(provider).cloned() else {
                continue;
            };
            if provider.get("api_key").and_then(Value::as_str) != Some(GUARD_API_KEY)
                && provider.get("base_url").and_then(Value::as_str) != Some(ACCOUNT_MODEL_GUARD_URL)
            {
                continue;
            }
            let source = entry.get("source");
            let ambiguous = provider
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| provider_counts.get(id))
                .is_some_and(|count| *count > 1);
            let version_matches = match entry.get("version").and_then(Value::as_u64) {
                Some(1) => {
                    source
                        .and_then(|provider| provider.pointer("/auth/type"))
                        .and_then(Value::as_str)
                        == Some("subscription")
                }
                Some(2) => {
                    source
                        .and_then(|provider| provider.pointer("/auth/type"))
                        .and_then(Value::as_str)
                        == Some("subscription_pool")
                }
                _ => false,
            };
            let restorable = !ambiguous
                && version_matches
                && source.is_some_and(|source| {
                    source.get("id") == provider.get("id")
                        && explicit_account_provider(source)
                        && serde_json::from_value::<AIProviderConfig>(source.clone()).is_ok()
                });
            if restorable {
                *provider = source.unwrap().clone();
            } else {
                make_provider_projection(provider, entry);
                diagnostics.push(ConfigDiagnostic {
                    path: format!("ai.providers[{index}]"),
                    message: if ambiguous {
                        "Multiple providers share this account binding's configuration ID; their original records were preserved and requests are blocked.".into()
                    } else {
                        "This subscription account binding is unreadable or requires a newer OpenBitFun; its original data was preserved and requests are blocked.".into()
                    },
                    code: "SUBSCRIPTION_ACCOUNT_BINDING_UNSUPPORTED".into(),
                    severity: ConfigDiagnosticSeverity::Warning,
                    recoverability: ConfigDiagnosticRecoverability::None,
                });
            }
        }
    }
    let model_counts = id_counts_at(value, "/ai/models");
    if let Some(models) = value
        .pointer_mut("/ai/models")
        .and_then(Value::as_array_mut)
    {
        for (index, model) in models.iter_mut().enumerate() {
            let Some(entry) = guard_entry(model).cloned() else {
                continue;
            };
            if model.get("channel_routing").is_some() {
                continue;
            }
            let source = entry.get("source");
            let ambiguous = model
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| model_counts.get(id))
                .is_some_and(|count| *count > 1);
            let version_matches = entry.get("version").and_then(Value::as_u64) == Some(3)
                && source
                    .and_then(|source| source.get("channel_routing"))
                    .is_some_and(|route| {
                        serde_json::from_value::<crate::channel_routing::ModelChannelRouting>(
                            route.clone(),
                        )
                        .is_ok_and(|route| route.version == 1)
                    });
            let restorable = !ambiguous
                && version_matches
                && source.is_some_and(|source| {
                    source.get("id") == model.get("id")
                        && explicit_account_entry(source)
                        && serde_json::from_value::<AIModelEntry>(source.clone()).is_ok()
                });
            if restorable {
                *model = source.unwrap().clone();
            } else {
                make_entry_projection(model, entry);
                diagnostics.push(ConfigDiagnostic {
                    path: format!("ai.models[{index}]"),
                    message: if ambiguous {
                        "Multiple models share this channel binding's configuration ID; their original records were preserved and requests are blocked.".into()
                    } else {
                        "This channel binding is unreadable or requires a newer OpenBitFun; its original data was preserved and requests are blocked.".into()
                    },
                    code: "SUBSCRIPTION_ACCOUNT_BINDING_UNSUPPORTED".into(),
                    severity: ConfigDiagnosticSeverity::Warning,
                    recoverability: ConfigDiagnosticRecoverability::None,
                });
            }
        }
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model_runtime_binding_fingerprint, GlobalConfig, SubscriptionProvider};
    use serde::{Deserialize, Serialize};

    fn config() -> Value {
        let mut config = GlobalConfig::default();
        config.ai.providers = vec![AIProviderConfig {
            id: "seat-two".into(),
            name: "Codex account".into(),
            provider: "responses".into(),
            base_url: "https://chatgpt.com/backend-api/codex".into(),
            api_key: String::new(),
            enabled: true,
            auth: AuthConfig::Subscription {
                provider: SubscriptionProvider::Codex,
                plan: None,
                account_id: Some("codex-account-58134808-15a6-45db-ae33-2ad7d85cc570".into()),
            },
            metadata: Some(json!({"future_field":{"keep":true}})),
            ..AIProviderConfig::default()
        }];
        config.ai.models = vec![AIModelEntry {
            id: "named-model".into(),
            provider_id: "seat-two".into(),
            model_name: "fixture-model".into(),
            capabilities: vec![crate::ModelCapability::TextChat],
            enabled: true,
            request_url: Some("https://chatgpt.com/backend-api/codex/responses".into()),
            metadata: Some(json!({"future_entry_field":{"keep":true}})),
            ..AIModelEntry::default()
        }];
        config.ai.default_models.primary = Some("named-model".into());
        serde_json::to_value(config).unwrap()
    }

    // Auth behavior from the released model shape: the legacy writer
    // understands only api_key auth and opaque metadata survives settings
    // writes.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum LegacyAuth {
        ApiKey,
        Subscription { provider: SubscriptionProvider },
    }

    #[test]
    fn new_subscription_defaults_survive_legacy_settings_round_trip() {
        let mut original = config();
        original["ai"]["providers"][0]["custom_headers"] =
            json!({"Authorization": "saved-inline-header"});
        original["ai"]["providers"][0]["custom_request_body"] = json!("{\"extra\":true}");
        for provider in ["workbuddy", "workbuddy_ai", "zcode", "zcode_cn"] {
            let mut original = original.clone();
            original["ai"]["providers"][0]["auth"] =
                json!({"type":"subscription","provider":provider});
            let mut persisted = original.clone();
            protect_account_models(&mut persisted);
            let outer = &persisted["ai"]["providers"][0];
            assert_eq!(outer["auth"], json!({"type":"api_key"}));
            assert_eq!(outer["base_url"], ACCOUNT_MODEL_GUARD_URL);
            assert_eq!(outer["custom_headers"], Value::Null);
            // An old settings writer preserves opaque metadata while changing
            // unrelated settings. The source auth cannot reach its public URL.
            let mut legacy: GlobalConfig = serde_json::from_value(persisted).unwrap();
            legacy.ai.default_models.primary = Some("named-model".into());
            let mut restored = serde_json::to_value(legacy).unwrap();
            assert!(restore_account_models(&mut restored).is_empty());
            assert_eq!(restored, original);
        }
        // Existing provider-default records keep their original on-disk shape.
        let mut old = config();
        old["ai"]["providers"][0]["auth"] = json!({"type":"subscription","provider":"codex"});
        let unchanged = old.clone();
        protect_account_models(&mut old);
        assert_eq!(old, unchanged);
    }

    #[test]
    fn channel_guards_prevent_legacy_primary_substitution_and_keep_unknown_fields_on_edit() {
        let mut original = config();
        original["ai"]["providers"][0]["auth"] = json!({"type":"api_key"});
        original["ai"]["providers"][0]["api_key"] = json!("synthetic-primary-key");
        let backup_provider = json!({
            "id": "backup-provider",
            "name": "Backup provider",
            "provider": "responses",
            "base_url": "https://backup.example.test/v1",
            "api_key": "synthetic-backup-key",
            "enabled": true
        });
        original["ai"]["providers"]
            .as_array_mut()
            .unwrap()
            .push(backup_provider);
        let backup = json!({
            "id": "backup",
            "provider_id": "backup-provider",
            "model_name": "backup-model",
            "enabled": true
        });
        original["ai"]["models"]
            .as_array_mut()
            .unwrap()
            .push(backup);
        original["ai"]["models"][0]["channel_routing"] = json!({"version":1,"fallback_model_ids":["backup"],"manual_channel":"backup","future_option":{"keep":true}});
        let mut protected = original.clone();
        protect_account_models(&mut protected);
        let outer = &protected["ai"]["models"][0];
        assert_eq!(outer["metadata"][ACCOUNT_MODEL_GUARD_KEY]["version"], 3);
        assert!(outer.get("channel_routing").is_none());
        assert_eq!(
            protected["ai"]["providers"][0]["base_url"],
            original["ai"]["providers"][0]["base_url"]
        );
        let sources = account_guard_sources(&protected);
        assert!(restore_account_models(&mut protected).is_empty());
        assert_eq!(protected, original);
        let mut typed: GlobalConfig = serde_json::from_value(protected).unwrap();
        typed.ai.models[0]
            .channel_routing
            .as_mut()
            .unwrap()
            .manual_channel = Some("named-model".into());
        let mut changed = serde_json::to_value(typed).unwrap();
        protect_account_models(&mut changed);
        preserve_account_guard_sources(&mut changed, &sources);
        let retained = &changed["ai"]["models"][0]["metadata"][ACCOUNT_MODEL_GUARD_KEY]["source"]
            ["channel_routing"];
        assert_eq!(retained["manual_channel"], "named-model");
        assert_eq!(retained["future_option"], json!({"keep":true}));
        let source = &mut changed["ai"]["models"][0]["metadata"][ACCOUNT_MODEL_GUARD_KEY]["source"];
        source["channel_routing"]["version"] = json!(99);
        let unknown = source.clone();
        assert_eq!(restore_account_models(&mut changed).len(), 1);
        assert_eq!(
            changed["ai"]["models"][0]["metadata"][ACCOUNT_MODEL_GUARD_KEY]["source"],
            unknown
        );
        assert_eq!(
            changed["ai"]["models"][0]["metadata"][ACCOUNT_MODEL_GUARD_KEY]["version"],
            json!(3)
        );
    }

    #[test]
    fn legacy_auth_roundtrip_cannot_select_a_default_account_and_preserves_binding() {
        let original = config();
        let before: GlobalConfig = serde_json::from_value(original.clone()).unwrap();
        let mut persisted = original.clone();
        protect_account_models(&mut persisted);
        let provider = &persisted["ai"]["providers"][0];
        let old_auth: LegacyAuth = serde_json::from_value(provider["auth"].clone()).unwrap();
        assert!(matches!(old_auth, LegacyAuth::ApiKey));
        assert_eq!(provider["base_url"], ACCOUNT_MODEL_GUARD_URL);
        assert_eq!(provider["api_key"], GUARD_API_KEY);
        assert_eq!(provider["custom_headers"], Value::Null);
        assert_eq!(provider["custom_request_body"], Value::Null);
        assert_eq!(provider["enabled"], true);
        assert_eq!(persisted["ai"]["models"][0]["enabled"], true);
        let mut old_write = persisted.clone();
        old_write["ai"]["providers"][0]["auth"] = serde_json::to_value(old_auth).unwrap();
        old_write["app"]["auto_update"] = json!(false);
        assert!(restore_account_models(&mut old_write).is_empty());
        assert_eq!(old_write["ai"]["providers"], original["ai"]["providers"]);
        assert_eq!(old_write["app"]["auto_update"], false);
        let after: GlobalConfig = serde_json::from_value(old_write).unwrap();
        assert_eq!(
            model_runtime_binding_fingerprint(
                &crate::load_resolved_models(&before.ai.providers, &before.ai.models)[0]
            ),
            model_runtime_binding_fingerprint(
                &crate::load_resolved_models(&after.ai.providers, &after.ai.models)[0]
            )
        );
        // Legacy enabled/reference facts must remain intact. Otherwise its
        // normalizer could clear the selected model and pick another account.
        let mut legacy_projection: GlobalConfig = serde_json::from_value(persisted).unwrap();
        crate::normalization::recover_persisted_config(&mut legacy_projection);
        assert_eq!(
            legacy_projection.ai.default_models.primary.as_deref(),
            Some("named-model")
        );
        assert!(legacy_projection.ai.models[0].enabled);
    }

    #[test]
    fn default_accounts_and_unrelated_models_keep_the_existing_format() {
        let mut value = config();
        value["ai"]["providers"][0]["auth"]
            .as_object_mut()
            .unwrap()
            .remove("account_id");
        let before = value.clone();
        protect_account_models(&mut value);
        assert_eq!(value, before);
        value["ai"]["providers"][0]["auth"] = json!({"type":"api_key"});
        let before = value.clone();
        protect_account_models(&mut value);
        assert_eq!(value, before);
    }

    #[test]
    fn unsupported_and_corrupt_guards_preserve_source_and_remain_unusable() {
        for source in [
            json!({"version":99,"source":{"future":"keep"}}),
            json!({"version":1,"source":{"id":"wrong"}}),
        ] {
            let mut value = config();
            protect_account_models(&mut value);
            value["ai"]["providers"][0]["metadata"][ACCOUNT_MODEL_GUARD_KEY] = source.clone();
            let diagnostics = restore_account_models(&mut value);
            assert_eq!(
                diagnostics[0].code,
                "SUBSCRIPTION_ACCOUNT_BINDING_UNSUPPORTED"
            );
            protect_account_models(&mut value);
            assert_eq!(
                value["ai"]["providers"][0]["metadata"][ACCOUNT_MODEL_GUARD_KEY],
                source
            );
            let resolved = &crate::load_resolved_models(
                &serde_json::from_value::<GlobalConfig>(value.clone())
                    .unwrap()
                    .ai
                    .providers,
                &serde_json::from_value::<GlobalConfig>(value.clone())
                    .unwrap()
                    .ai
                    .models,
            )[0];
            assert!(is_guarded_account_model(resolved));
            assert!(crate::conversion::AIConfig::try_from(resolved.clone())
                .unwrap_err()
                .contains("account-capable"));
        }
    }

    #[test]
    fn ambiguous_ids_never_merge_or_discard_distinct_source_records() {
        let mut value = config();
        let mut duplicate = value["ai"]["providers"][0].clone();
        duplicate["name"] = json!("Different provider with same ID");
        value["ai"]["providers"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        protect_account_models(&mut value);
        for (index, provider) in value["ai"]["providers"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            provider["metadata"][ACCOUNT_MODEL_GUARD_KEY]["future"] = json!(index);
        }
        let sources = account_guard_sources(&value);
        let before: Vec<_> = value["ai"]["providers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|provider| guard_entry(provider).unwrap().clone())
            .collect();
        assert_eq!(restore_account_models(&mut value).len(), 2);
        let typed: GlobalConfig = serde_json::from_value(value).unwrap();
        let mut rewritten = serde_json::to_value(typed).unwrap();
        protect_account_models(&mut rewritten);
        preserve_account_guard_sources(&mut rewritten, &sources);
        for (index, provider) in rewritten["ai"]["providers"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            assert_eq!(guard_entry(provider).unwrap(), &before[index]);
        }
    }

    #[test]
    fn account_pools_use_version_two_and_roundtrip_without_legacy_default_auth() {
        let mut value = config();
        value["ai"]["providers"][0]["auth"] = json!({"type":"subscription_pool","provider":"codex","accounts":["codex","codex-account-58134808-15a6-45db-ae33-2ad7d85cc570"],"revision":2});
        let original = value.clone();
        protect_account_models(&mut value);
        assert_eq!(
            value["ai"]["providers"][0]["metadata"][ACCOUNT_MODEL_GUARD_KEY]["version"],
            2
        );
        let old_auth: LegacyAuth =
            serde_json::from_value(value["ai"]["providers"][0]["auth"].clone()).unwrap();
        assert!(matches!(old_auth, LegacyAuth::ApiKey));
        assert!(restore_account_models(&mut value).is_empty());
        assert_eq!(value, original);
    }
}
