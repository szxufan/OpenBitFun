use crate::types::{
    ConfigDiagnostic, ConfigDiagnosticRecoverability, ConfigDiagnosticSeverity, GlobalConfig,
    ModelCapability, SubagentModelSelection,
};
use std::collections::HashSet;

/// Custom wire messages bypass the host's image selection and cannot be mixed
/// with a host-enforced image budget. Other request options remain editable.
pub(crate) fn validate_image_budget_request_body(
    max_images_per_request: Option<usize>,
    body: Option<&serde_json::Value>,
) -> Result<(), &'static str> {
    if max_images_per_request.is_some()
        && body
            .and_then(serde_json::Value::as_object)
            .is_some_and(|body| {
                ["messages", "input", "contents"]
                    .iter()
                    .any(|key| body.contains_key(*key))
            })
    {
        return Err("custom_request_body cannot override messages, input, or contents when max_images_per_request is configured");
    }
    Ok(())
}

/// Provider-neutral provider validation, shared by startup and offline import.
pub fn validate_provider_config(
    provider: &crate::AIProviderConfig,
    index: usize,
) -> Result<(), String> {
    if provider.provider.trim().is_empty() {
        return Err(format!("Provider protocol is required at index {index}"));
    }
    Ok(())
}

/// Provider-neutral model entry validation, shared by startup and offline
/// import. Rules spanning an entry and its provider (the image request budget
/// against the provider's custom request body) are checked by
/// [`validate_resolved_model_config`].
pub fn validate_model_entry(entry: &crate::AIModelEntry, index: usize) -> Result<(), String> {
    if !entry.supports_text_generation() {
        for (field, present) in [
            ("context_window", entry.context_window.is_some()),
            ("max_tokens", entry.max_tokens.is_some()),
            ("temperature", entry.temperature.is_some()),
            ("top_p", entry.top_p.is_some()),
        ] {
            if present {
                return Err(format!(
                    "Model '{}' has text-generation-only field {field} at index {index}",
                    entry.model_name
                ));
            }
        }
    }
    if !entry.enabled {
        return Ok(());
    }
    if entry.max_images_per_request == Some(0) {
        return Err(format!(
            "Model '{}' max_images_per_request must be greater than 0 at index {index}",
            entry.model_name
        ));
    }
    if entry.provider_id.trim().is_empty() {
        return Err(format!(
            "Model '{}' provider_id is required at index {index}",
            entry.model_name
        ));
    }
    if entry.supports_text_generation() {
        if entry
            .context_window
            .is_some_and(|size| size < crate::MIN_MODEL_CONTEXT_WINDOW_TOKENS)
        {
            return Err(format!(
                "Model '{}' context_window must be at least {} at index {index}",
                entry.model_name,
                crate::MIN_MODEL_CONTEXT_WINDOW_TOKENS
            ));
        }
        if entry.max_tokens == Some(0) {
            return Err(format!(
                "Model '{}' max_tokens must be greater than 0 at index {index}",
                entry.model_name
            ));
        }
    }
    if let Some(reasoning) = &entry.reasoning {
        reasoning.validate_schema().map_err(|error| {
            format!(
                "Model '{}' reasoning config is invalid at index {index}: {error}",
                entry.model_name
            )
        })?;
    }
    Ok(())
}

/// Validate the resolved projection of one entry. The image request budget is
/// enforced against the owning provider's custom request body, so this needs
/// the join result rather than the bare entry.
pub fn validate_resolved_model_config(
    model: &crate::AIModelConfig,
    index: usize,
) -> Result<(), String> {
    if model.max_images_per_request == Some(0) {
        return Err(format!(
            "Model '{}' max_images_per_request must be greater than 0 at index {index}",
            model.model_name
        ));
    }
    if model.max_images_per_request.is_some() {
        let body = model
            .custom_request_body
            .as_deref()
            .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok());
        validate_image_budget_request_body(model.max_images_per_request, body.as_ref())
            .map_err(|error| format!("Model '{}' {error} at index {index}", model.model_name))?;
    }
    Ok(())
}

/// Recover old persisted values without dropping model records or credentials.
/// The caller owns reporting and persistence; this function performs no IO.
pub fn recover_persisted_config(config: &mut GlobalConfig) -> Vec<ConfigDiagnostic> {
    let mut diagnostics = normalize_typed_config(config);
    let defaults = GlobalConfig::default();
    let mut repaired = |path: &str| {
        diagnostics.push(ConfigDiagnostic {
            path: path.into(),
            message: "An invalid persisted value was replaced with the current default.".into(),
            code: "CONFIG_VALUE_RECOVERED".into(),
            severity: ConfigDiagnosticSeverity::Warning,
            recoverability: ConfigDiagnosticRecoverability::DefaultsUsed,
        })
    };
    if config.ai.stream_idle_timeout_secs == Some(0) {
        config.ai.stream_idle_timeout_secs = defaults.ai.stream_idle_timeout_secs;
        repaired("ai.stream_idle_timeout_secs");
    }
    if config.ai.stream_ttft_timeout_secs == Some(0) {
        config.ai.stream_ttft_timeout_secs = defaults.ai.stream_ttft_timeout_secs;
        repaired("ai.stream_ttft_timeout_secs");
    }
    if config.appearance.selection.trim().is_empty() {
        config.appearance.selection = defaults.appearance.selection;
        repaired("appearance.selection");
    }
    if !matches!(
        config.app.logging.level.to_lowercase().as_str(),
        "trace" | "debug" | "info" | "warn" | "error" | "off"
    ) {
        config.app.logging.level = defaults.app.logging.level;
        repaired("app.logging.level");
    }
    for (index, provider) in config.ai.providers.iter_mut().enumerate() {
        if let Err(message) = validate_provider_config(provider, index) {
            provider.enabled = false;
            diagnostics.push(ConfigDiagnostic {
                path: format!("ai.providers[{index}]"),
                message,
                code: "CONFIG_PROVIDER_DISABLED".into(),
                severity: ConfigDiagnosticSeverity::Warning,
                recoverability: ConfigDiagnosticRecoverability::ModelDisabled,
            });
        }
    }
    let provider_ids: HashSet<&str> = config
        .ai
        .providers
        .iter()
        .map(|provider| provider.id.as_str())
        .collect();
    for (index, model) in config.ai.models.iter_mut().enumerate() {
        let mut message = validate_model_entry(model, index).err();
        // A dangling reference only matters while the entry can still run;
        // already-disabled entries keep their record without new diagnostics.
        if message.is_none() && model.enabled && !provider_ids.contains(model.provider_id.as_str())
        {
            message = Some(format!(
                "Model '{}' references missing provider '{}' at index {index}",
                model.model_name, model.provider_id
            ));
        }
        if let Some(message) = message {
            model.enabled = false;
            diagnostics.push(ConfigDiagnostic {
                path: format!("ai.models[{index}]"),
                message,
                code: "CONFIG_MODEL_DISABLED".into(),
                severity: ConfigDiagnosticSeverity::Warning,
                recoverability: ConfigDiagnosticRecoverability::ModelDisabled,
            });
        }
    }
    diagnostics.extend(reconcile_model_references(config).diagnostics);
    diagnostics
}

/// Canonicalizes typed model fields whose meaning is capability-dependent.
pub fn normalize_typed_config(config: &mut GlobalConfig) -> Vec<ConfigDiagnostic> {
    let mut diagnostics = Vec::new();

    for (index, model) in config.ai.models.iter_mut().enumerate() {
        model.ensure_category_and_capabilities();
        let model_id = model.id.clone();
        for field in model.normalize_inapplicable_generation_fields() {
            diagnostics.push(ConfigDiagnostic {
                path: format!("ai.models[{index}].{field}"),
                message: format!(
                    "Cleared text-generation-only field from model '{}' because it does not support text_chat",
                    model_id
                ),
                code: "MODEL_FIELD_NOT_APPLICABLE".to_string(),
                severity: ConfigDiagnosticSeverity::Warning,
                recoverability: ConfigDiagnosticRecoverability::AutoFix,
            });
        }
    }

    diagnostics
}

#[derive(Debug, Clone, Default)]
pub struct ModelReferenceReconcileResult {
    pub invalidated_model_ids: Vec<String>,
    pub default_models_changed: bool,
    pub task_models_changed: bool,
    pub agent_model_defaults_changed: bool,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

impl ModelReferenceReconcileResult {
    pub fn is_noop(&self) -> bool {
        !self.default_models_changed
            && !self.task_models_changed
            && !self.agent_model_defaults_changed
    }
}

fn provider_enabled(config: &GlobalConfig, provider_id: &str) -> bool {
    config
        .ai
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .is_some_and(|provider| provider.enabled)
}

fn enabled_model_with_capability(
    config: &GlobalConfig,
    model_id: &str,
    capability: ModelCapability,
) -> bool {
    config.ai.models.iter().any(|model| {
        model.enabled
            && provider_enabled(config, &model.provider_id)
            && model.id == model_id
            && model.supports_capability(capability.clone())
    })
}

fn first_enabled_model_with_capability(
    config: &GlobalConfig,
    capability: ModelCapability,
) -> Option<String> {
    config
        .ai
        .models
        .iter()
        .find(|model| {
            model.enabled
                && provider_enabled(config, &model.provider_id)
                && model.supports_capability(capability.clone())
        })
        .map(|model| model.id.clone())
}

fn diagnose_reference_repair(
    diagnostics: &mut Vec<ConfigDiagnostic>,
    path: &str,
    previous: Option<&str>,
    replacement: Option<&str>,
) {
    diagnostics.push(ConfigDiagnostic {
        path: path.to_string(),
        message: format!(
            "Repaired model reference from {:?} to {:?} to match the slot capability",
            previous, replacement
        ),
        code: "MODEL_REFERENCE_REPAIRED".to_string(),
        severity: ConfigDiagnosticSeverity::Warning,
        recoverability: ConfigDiagnosticRecoverability::AutoFix,
    });
}

#[derive(Clone, Copy)]
enum MissingSlotPolicy {
    FillFromFirstCapableModel,
    Preserve,
}

/// Reconciles every product model reference against both enablement and the
/// capability required by its consumer.
pub fn reconcile_model_references(config: &mut GlobalConfig) -> ModelReferenceReconcileResult {
    let snapshot = config.clone();
    let mut result = ModelReferenceReconcileResult::default();
    let mut invalidated = HashSet::new();

    let direct_text_reference_is_valid = |reference: &str| {
        matches!(reference, "primary" | "fast")
            || enabled_model_with_capability(&snapshot, reference, ModelCapability::TextChat)
    };

    let mut reconcile_task_model =
        |selection: &mut crate::types::TaskModelSelection, path: &str, allow_inherit: bool| {
            let previous = selection.fixed_model_id().map(str::to_string);
            let valid = match selection {
                crate::types::TaskModelSelection::Inherit => allow_inherit,
                crate::types::TaskModelSelection::Fixed { model_id } => {
                    direct_text_reference_is_valid(model_id)
                }
            };
            if !valid {
                if let Some(model_id) = previous.as_ref() {
                    invalidated.insert(model_id.clone());
                }
                *selection = crate::types::TaskModelSelection::Fixed {
                    model_id: "fast".to_string(),
                };
                result.task_models_changed = true;
                diagnose_reference_repair(
                    &mut result.diagnostics,
                    path,
                    previous.as_deref().or(Some("inherit")),
                    Some("fast"),
                );
            }
        };
    reconcile_task_model(
        &mut config.ai.task_models.session_title,
        "ai.task_models.session_title",
        true,
    );
    reconcile_task_model(
        &mut config.ai.task_models.git_commit,
        "ai.task_models.git_commit",
        false,
    );

    if config.ai.agent_model_selections.is_none() {
        if !direct_text_reference_is_valid(&config.ai.agent_model_defaults.mode) {
            invalidated.insert(config.ai.agent_model_defaults.mode.clone());
            let previous = std::mem::replace(
                &mut config.ai.agent_model_defaults.mode,
                "primary".to_string(),
            );
            result.agent_model_defaults_changed = true;
            diagnose_reference_repair(
                &mut result.diagnostics,
                "ai.agent_model_defaults.mode",
                Some(&previous),
                Some("primary"),
            );
        }

        if config
            .ai
            .agent_model_defaults
            .subagents
            .default_selection
            .fixed_model_id()
            .is_some_and(|model_id| !direct_text_reference_is_valid(model_id))
        {
            let previous = config
                .ai
                .agent_model_defaults
                .subagents
                .default_selection
                .fixed_model_id()
                .map(str::to_string);
            if let Some(previous) = previous.as_ref() {
                invalidated.insert(previous.clone());
            }
            config.ai.agent_model_defaults.subagents.default_selection =
                SubagentModelSelection::fixed("fast");
            result.agent_model_defaults_changed = true;
            diagnose_reference_repair(
                &mut result.diagnostics,
                "ai.agent_model_defaults.subagents.default",
                previous.as_deref(),
                Some("fast"),
            );
        }

        if let std::collections::hash_map::Entry::Vacant(entry) = config
            .ai
            .agent_model_defaults
            .subagents
            .builtin
            .entry("ResearchSpecialist".to_string())
        {
            entry.insert(SubagentModelSelection::Inherit);
            result.agent_model_defaults_changed = true;
            result.diagnostics.push(ConfigDiagnostic {
            path: "ai.agent_model_defaults.subagents.builtin.ResearchSpecialist".to_string(),
            message: "Configured ResearchSpecialist to inherit the parent model so DeepResearch does not depend on an unrelated fast-model endpoint".to_string(),
            code: "RESEARCH_SPECIALIST_MODEL_DEFAULT_RESTORED".to_string(),
            severity: ConfigDiagnosticSeverity::Warning,
            recoverability: ConfigDiagnosticRecoverability::AutoFix,
        });
        }

        config
            .ai
            .agent_model_defaults
            .subagents
            .builtin
            .retain(|subagent_id, selection| {
                let invalid = selection
                    .fixed_model_id()
                    .is_some_and(|model_id| !direct_text_reference_is_valid(model_id));
                if invalid {
                    if let Some(model_id) = selection.fixed_model_id() {
                        invalidated.insert(model_id.to_string());
                        diagnose_reference_repair(
                            &mut result.diagnostics,
                            &format!("ai.agent_model_defaults.subagents.builtin.{subagent_id}"),
                            Some(model_id),
                            None,
                        );
                    }
                    result.agent_model_defaults_changed = true;
                }
                !invalid
            });

        if config
            .ai
            .agent_model_defaults
            .subagents
            .fork
            .fixed_model_id()
            .is_some_and(|model_id| !direct_text_reference_is_valid(model_id))
        {
            let previous = config
                .ai
                .agent_model_defaults
                .subagents
                .fork
                .fixed_model_id()
                .map(str::to_string);
            if let Some(previous) = previous.as_ref() {
                invalidated.insert(previous.clone());
            }
            config.ai.agent_model_defaults.subagents.fork = SubagentModelSelection::Inherit;
            result.agent_model_defaults_changed = true;
            diagnose_reference_repair(
                &mut result.diagnostics,
                "ai.agent_model_defaults.subagents.fork",
                previous.as_deref(),
                Some("inherit"),
            );
        }
    }

    let mut reconcile_slot = |slot: &mut Option<String>,
                              path: &str,
                              capability: ModelCapability,
                              missing_policy: MissingSlotPolicy| {
        let previous = slot.clone();
        let valid = previous
            .as_deref()
            .is_some_and(|id| enabled_model_with_capability(&snapshot, id, capability.clone()));
        if valid || (previous.is_none() && matches!(missing_policy, MissingSlotPolicy::Preserve)) {
            return;
        }
        let replacement = first_enabled_model_with_capability(&snapshot, capability);
        if replacement == previous {
            return;
        }
        if let Some(previous) = previous.as_ref().filter(|id| !id.is_empty()) {
            invalidated.insert(previous.clone());
        }
        *slot = replacement;
        result.default_models_changed = true;
        diagnose_reference_repair(
            &mut result.diagnostics,
            path,
            previous.as_deref(),
            slot.as_deref(),
        );
    };

    reconcile_slot(
        &mut config.ai.default_models.primary,
        "ai.default_models.primary",
        ModelCapability::TextChat,
        MissingSlotPolicy::FillFromFirstCapableModel,
    );
    reconcile_slot(
        &mut config.ai.default_models.fast,
        "ai.default_models.fast",
        ModelCapability::TextChat,
        MissingSlotPolicy::Preserve,
    );
    reconcile_slot(
        &mut config.ai.default_models.image_understanding,
        "ai.default_models.image_understanding",
        ModelCapability::ImageUnderstanding,
        MissingSlotPolicy::Preserve,
    );
    reconcile_slot(
        &mut config.ai.default_models.image_generation,
        "ai.default_models.image_generation",
        ModelCapability::ImageGeneration,
        MissingSlotPolicy::Preserve,
    );
    reconcile_slot(
        &mut config.ai.default_models.search,
        "ai.default_models.search",
        ModelCapability::Search,
        MissingSlotPolicy::Preserve,
    );
    reconcile_slot(
        &mut config.ai.default_models.speech_recognition,
        "ai.default_models.speech_recognition",
        ModelCapability::SpeechRecognition,
        MissingSlotPolicy::Preserve,
    );

    result.invalidated_model_ids = invalidated.into_iter().collect();
    result.invalidated_model_ids.sort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AIModelEntry, ModelCapability, ModelCategory};

    fn test_entry(
        id: &str,
        category: ModelCategory,
        capabilities: Vec<ModelCapability>,
    ) -> AIModelEntry {
        AIModelEntry {
            id: id.to_string(),
            provider_id: "prov".to_string(),
            model_name: id.to_string(),
            enabled: true,
            category,
            capabilities,
            ..AIModelEntry::default()
        }
    }

    #[test]
    fn recovery_preserves_models_and_repairs_all_invalid_reference_families() {
        let mut value = serde_json::to_value(GlobalConfig::default()).unwrap();
        let patch = serde_json::json!({"ai": {
            "stream_idle_timeout_secs": 0, "stream_ttft_timeout_secs": 0,
            "providers": [
                {"id":"prov", "name":"Prov", "provider":"openai", "base_url":"https://prov.test/v1", "api_key":"retained-secret", "enabled":true}
            ],
            "models": [
                {"id":"text", "provider_id":"prov", "model_name":"text", "enabled":true},
                {"id":"speech", "provider_id":"prov", "model_name":"speech", "enabled":true,
                 "capabilities":["speech_recognition"], "context_window":0, "max_tokens":0, "temperature":1.0, "top_p":1.0},
                {"id":"broken", "provider_id":"missing", "model_name":"broken", "enabled":true}
            ],
            "default_models":{"primary":"missing", "fast":"broken", "search":"text", "speech_recognition":"text"},
            "task_models":{"session_title":{"kind":"fixed","model_id":"speech"}, "git_commit":{"kind":"inherit"}},
            "agent_model_defaults":{"mode":"missing", "subagents":{
                "default":{"kind":"fixed","model_id":"missing-model"},
                "fork":{"kind":"fixed","model_id":"broken"},
                "builtin":{"Explore":{"kind":"fixed","model_id":"missing"}}
            }}
        }, "app":{"logging":{"level":"invalid"}}, "appearance":{"selection":" "}});
        for (key, field) in patch.as_object().unwrap() {
            value[key] = field.clone();
        }
        let mut config: GlobalConfig = serde_json::from_value(value).unwrap();
        let diagnostics = recover_persisted_config(&mut config);
        assert!(!diagnostics.is_empty());
        assert_eq!(config.ai.models.len(), 3);
        assert!(!config.ai.models[2].enabled);
        assert_eq!(config.ai.providers[0].api_key, "retained-secret");
        assert!(config.ai.models[1].enabled);
        assert_eq!(config.ai.models[1].context_window, None);
        assert_eq!(config.ai.models[1].top_p, None);
        assert_eq!(config.ai.default_models.primary.as_deref(), Some("text"));
        assert_eq!(config.ai.default_models.fast.as_deref(), Some("text"));
        assert_eq!(config.ai.default_models.search, None);
        assert_eq!(
            config.ai.default_models.speech_recognition.as_deref(),
            Some("speech")
        );
        assert_eq!(
            config.ai.task_models.session_title.fixed_model_id(),
            Some("fast")
        );
        assert_eq!(
            config.ai.task_models.git_commit.fixed_model_id(),
            Some("fast")
        );
        assert_eq!(config.ai.agent_model_defaults.mode, "primary");
        assert_eq!(
            config
                .ai
                .agent_model_defaults
                .subagents
                .default_selection
                .fixed_model_id(),
            Some("fast")
        );
        assert_eq!(
            config.ai.agent_model_defaults.subagents.fork,
            SubagentModelSelection::Inherit
        );
        assert!(!config
            .ai
            .agent_model_defaults
            .subagents
            .builtin
            .contains_key("Explore"));
        assert!(recover_persisted_config(&mut config).is_empty());
        let saved = serde_json::to_value(&config).unwrap();
        let mut reloaded: GlobalConfig = serde_json::from_value(saved.clone()).unwrap();
        assert!(recover_persisted_config(&mut reloaded).is_empty());
        assert_eq!(serde_json::to_value(reloaded).unwrap(), saved);
    }
    #[test]
    fn pure_speech_models_drop_text_generation_sentinels() {
        let mut config = GlobalConfig::default();
        config.ai.models.push(AIModelEntry {
            context_window: Some(0),
            max_tokens: Some(0),
            ..test_entry(
                "speech-cloud",
                ModelCategory::SpeechRecognition,
                vec![ModelCapability::SpeechRecognition],
            )
        });

        let diagnostics = normalize_typed_config(&mut config);

        assert_eq!(config.ai.models[0].context_window, None);
        assert_eq!(config.ai.models[0].max_tokens, None);
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "MODEL_FIELD_NOT_APPLICABLE"));
    }

    #[test]
    fn mixed_text_and_speech_models_keep_generation_fields() {
        let mut config = GlobalConfig::default();
        config.ai.models.push(AIModelEntry {
            context_window: Some(64_000),
            max_tokens: Some(8_000),
            ..test_entry(
                "mixed",
                ModelCategory::GeneralChat,
                vec![
                    ModelCapability::TextChat,
                    ModelCapability::SpeechRecognition,
                ],
            )
        });

        assert!(normalize_typed_config(&mut config).is_empty());
        assert_eq!(config.ai.models[0].context_window, Some(64_000));
        assert_eq!(config.ai.models[0].max_tokens, Some(8_000));
    }

    #[test]
    fn default_slots_reconcile_by_capability() {
        let mut config = GlobalConfig::default();
        config.ai.providers.push(crate::AIProviderConfig {
            id: "prov".into(),
            name: "Prov".into(),
            ..Default::default()
        });
        config.ai.models = vec![
            test_entry(
                "speech",
                ModelCategory::SpeechRecognition,
                vec![ModelCapability::SpeechRecognition],
            ),
            test_entry(
                "text",
                ModelCategory::GeneralChat,
                vec![ModelCapability::TextChat],
            ),
        ];
        config.ai.default_models.primary = Some("speech".to_string());
        config.ai.default_models.fast = Some("speech".to_string());
        config.ai.default_models.speech_recognition = Some("text".to_string());

        let result = reconcile_model_references(&mut config);

        assert_eq!(config.ai.default_models.primary.as_deref(), Some("text"));
        assert_eq!(config.ai.default_models.fast.as_deref(), Some("text"));
        assert_eq!(
            config.ai.default_models.speech_recognition.as_deref(),
            Some("speech")
        );
        assert!(result.default_models_changed);
    }

    #[test]
    fn missing_fast_slot_is_preserved_and_resolves_to_primary() {
        let mut config = GlobalConfig::default();
        config.ai.providers.push(crate::AIProviderConfig {
            id: "prov".into(),
            name: "Prov".into(),
            ..Default::default()
        });
        config.ai.models = vec![
            test_entry(
                "first-text",
                ModelCategory::GeneralChat,
                vec![ModelCapability::TextChat],
            ),
            test_entry(
                "primary-text",
                ModelCategory::GeneralChat,
                vec![ModelCapability::TextChat],
            ),
        ];
        config.ai.default_models.primary = Some("primary-text".to_string());
        config.ai.default_models.fast = None;

        let result = reconcile_model_references(&mut config);

        assert_eq!(config.ai.default_models.fast, None);
        assert_eq!(
            config.ai.resolve_model_selection("fast").as_deref(),
            Some("primary-text")
        );
        assert!(!result.default_models_changed);
    }

    #[test]
    fn task_models_reconcile_invalid_fixed_and_git_inherit_to_fast() {
        let mut config = GlobalConfig::default();
        config.ai.task_models.session_title = crate::types::TaskModelSelection::Fixed {
            model_id: "missing".to_string(),
        };
        config.ai.task_models.git_commit = crate::types::TaskModelSelection::Inherit;

        let result = reconcile_model_references(&mut config);

        assert!(result.task_models_changed);
        assert_eq!(
            config.ai.task_models.session_title.fixed_model_id(),
            Some("fast")
        );
        assert_eq!(
            config.ai.task_models.git_commit.fixed_model_id(),
            Some("fast")
        );
        assert!(result
            .invalidated_model_ids
            .contains(&"missing".to_string()));
    }

    #[test]
    fn missing_research_specialist_default_is_restored_for_current_writes() {
        let mut config = GlobalConfig::default();
        config
            .ai
            .agent_model_defaults
            .subagents
            .builtin
            .remove("ResearchSpecialist");

        let result = reconcile_model_references(&mut config);

        assert!(result.agent_model_defaults_changed);
        assert_eq!(
            config
                .ai
                .agent_model_defaults
                .subagents
                .builtin
                .get("ResearchSpecialist"),
            Some(&SubagentModelSelection::Inherit)
        );
        assert!(result
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "RESEARCH_SPECIALIST_MODEL_DEFAULT_RESTORED" }));
    }

    #[test]
    fn explicit_research_specialist_model_override_is_preserved() {
        let mut config = GlobalConfig::default();
        config.ai.agent_model_defaults.subagents.builtin.insert(
            "ResearchSpecialist".to_string(),
            SubagentModelSelection::fixed("fast"),
        );

        let result = reconcile_model_references(&mut config);

        assert!(!result.agent_model_defaults_changed);
        assert_eq!(
            config
                .ai
                .agent_model_defaults
                .subagents
                .builtin
                .get("ResearchSpecialist"),
            Some(&SubagentModelSelection::fixed("fast"))
        );
    }
}
