import io

p = 'src/crates/assembly/core/src/service/config/manager.rs'
c = io.open(p, encoding='utf-8').read()

old = '''        reject_protected_metadata_path(path)?;
        if path == "ai.agent_model_defaults" || path.starts_with("ai.agent_model_defaults.") {
            return Err(OpenBitFunError::validation(
                "Legacy Agent model defaults are read-only; use ai.agent_model_selections",
            ));
        }'''
new = '''        reject_protected_metadata_path(path)?;
        if path == "ai.agent_model_defaults" || path.starts_with("ai.agent_model_defaults.") {
            return Err(OpenBitFunError::validation(
                "Legacy Agent model defaults are read-only; use ai.agent_model_selections",
            ));
        }
        // Every ai.models write funnels through here (direct settings, the CAS
        // mutation command, agent tools). Flattened legacy records are split
        // into slim entries plus provider records before validation, so the
        // two tables are always consistent inside one candidate commit.
        if path == "ai.models" {
            if let Some(value) = json_value.as_array() {
                let mut providers = self.config.ai.providers.clone();
                let mut entries = Vec::with_capacity(value.len());
                for record in value {
                    if record.get("provider_id").and_then(serde_json::Value::as_str)
                        .is_some_and(|id| !id.trim().is_empty())
                    {
                        entries.push(record.clone());
                        continue;
                    }
                    match serde_json::from_value::<openbitfun_config_contracts::AIModelConfig>(record.clone()) {
                        Ok(model) => {
                            entries.push(serde_json::to_value(
                                openbitfun_config_contracts::split_flattened_model(
                                    &model,
                                    &mut providers,
                                ),
                            ).map_err(|e| {
                                OpenBitFunError::config(format!(
                                    "Failed to normalize model configuration: {e}"
                                ))
                            })?);
                        }
                        // Unparseable records keep flowing so validation owns
                        // the user-facing error.
                        Err(_) => entries.push(record.clone()),
                    }
                }
                let mut config = self.config.clone();
                config.ai.providers = providers;
                config.ai.models = serde_json::from_value(
                    serde_json::Value::Array(entries),
                )
                .map_err(|e| {
                    OpenBitFunError::config(format!(
                        "Failed to normalize model configuration: {e}"
                    ))
                })?;
                normalize_typed_config(&mut config);
                reconcile_model_references(&mut config);
                let validation_result = self.providers.validate_config(&config).await?;
                if !validation_result.valid {
                    return Err(invalid_config_error(
                        "Invalid configuration update",
                        &validation_result,
                    ));
                }
                return self
                    .commit_candidate(
                        ConfigCandidate {
                            config,
                            guard_sources: self.account_guard_sources.clone(),
                            diagnostics: vec![],
                        },
                        Some(path),
                    )
                    .await;
            }
        }'''
assert old in c, 'manager.set head not found'
c = c.replace(old, new)
io.open(p, 'w', encoding='utf-8', newline='\n').write(c)
print('manager ok')

# Remove the broken normalization block from service.set_config_if_changed.
p = 'src/crates/assembly/core/src/service/config/service.rs'
c = io.open(p, encoding='utf-8').read()
old = '''            let before = manager.get::<serde_json::Value>(comparison_path).ok();
            if path == "ai.models" {
                // Legacy writers (external agents, sync payloads) submit
                // flattened model records. Normalize them into slim entries
                // plus provider records in one transaction.
                let mut providers: Vec<openbitfun_config_contracts::AIProviderConfig> = manager
                    .get::<Vec<openbitfun_config_contracts::AIProviderConfig>>("ai.providers")
                    .unwrap_or_default();
                let mut entries = Vec::new();
                if let Some(records) = value.as_array() {
                    for record in records {
                        if record.get("provider_id").and_then(serde_json::Value::as_str)
                            .is_some_and(|id| !id.trim().is_empty())
                        {
                            entries.push(record.clone());
                            continue;
                        }
                        match serde_json::from_value::<AIModelConfig>(record.clone()) {
                            Ok(model) => {
                                entries.push(serde_json::to_value(
                                    openbitfun_config_contracts::split_flattened_model(
                                        &model,
                                        &mut providers,
                                    ),
                                )?);
                            }
                            Err(_) => entries.push(record.clone()),
                        }
                    }
                }
                manager.set("ai.providers", providers).await?;
                manager.set(path, serde_json::Value::Array(entries)).await?;
            } else {
                manager.set(path, value).await?;
            }
            if before == manager.get::<serde_json::Value>(comparison_path).ok() {'''
new = '''            let before = manager.get::<serde_json::Value>(comparison_path).ok();
            manager.set(path, value).await?;
            if before == manager.get::<serde_json::Value>(comparison_path).ok() {'''
assert old in c, 'service block not found'
c = c.replace(old, new)
io.open(p, 'w', encoding='utf-8', newline='\n').write(c)
print('service ok')
