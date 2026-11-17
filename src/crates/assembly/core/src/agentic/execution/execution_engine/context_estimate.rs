//! On-demand inspection uses the request projection, without model execution or writes.
use super::*;
use crate::agentic::session::session_manager::ContextEstimateInputs;
use openbitfun_core_types::{UsageContextEstimate, UsageContextEstimateRequest};

impl ExecutionEngine {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn estimate_current_context(
        &self,
        inputs: ContextEstimateInputs,
        workspace: crate::agentic::WorkspaceBinding,
        workspace_services: Option<crate::agentic::workspace::WorkspaceServices>,
        terminal_port: Option<&Arc<dyn openbitfun_runtime_ports::TerminalPort>>,
        remote_exec_port: Option<&Arc<dyn openbitfun_runtime_ports::RemoteExecPort>>,
        restrictions: ToolRuntimeRestrictions,
        mut vars: HashMap<String, String>,
        options: &UsageContextEstimateRequest,
    ) -> OpenBitFunResult<UsageContextEstimate> {
        let session = &inputs.session;
        if session.config.agent_route_owner != openbitfun_core_types::SessionAgentRouteOwner::Local
            || session.agent_type.starts_with("acp:")
        {
            return Err(OpenBitFunError::NotImplemented(
                "The external agent does not expose its current model context".into(),
            ));
        }
        if session.kind != openbitfun_core_types::SessionKind::Standard {
            return Err(OpenBitFunError::NotImplemented(
                "Delegated session context requires its execution-scoped tool restrictions".into(),
            ));
        }
        let registry = get_agent_registry();
        // Match the remote-aware registry path; never discover controller-local project agents.
        registry
            .load_custom_agents(
                (!workspace.is_remote())
                    .then_some(workspace.workspace_id.as_deref())
                    .flatten(),
            )
            .await;
        let agent = registry
            .get_agent(&session.agent_type, workspace.workspace_id.as_deref())
            .ok_or_else(|| {
                OpenBitFunError::NotFound(format!("Agent not found: {}", session.agent_type))
            })?;
        let ai_config = SessionManager::load_ai_config_for_session(&session.config)
            .await?
            .ok_or_else(|| {
                OpenBitFunError::service(
                    "Model configuration is unavailable for context inspection",
                )
            })?;
        let (model_id, pool_window) =
            if session.config.model_binding_policy == SessionModelBindingPolicy::Pool {
                let pool_id = session.config.model_pool_id.as_deref().ok_or_else(|| {
                    OpenBitFunError::validation("Pool session has no pool selection")
                })?;
                // preview does not reserve a member or advance the pool cursor.
                let (id, _) = self
                    .model_pools
                    .preview(
                        &session.session_id,
                        session.config.model_pool_selection_revision,
                        pool_id,
                        session.config.enable_tools,
                        ImageInputRequirement::None,
                    )
                    .await?;
                (
                    id,
                    Some(
                        self.model_pools
                            .context_budget(pool_id)
                            .await?
                            .context_window as usize,
                    ),
                )
            } else {
                let (id, _) = self
                    .resolve_model_id_for_turn(
                        session,
                        &session.agent_type,
                        Some(&workspace),
                        inputs.turn_index,
                        None,
                        None,
                        ImageInputRequirement::None,
                    )
                    .await?;
                (id, None)
            };
        let model = ai_config
            .models
            .iter()
            .find(|model| model.id == model_id && model.enabled)
            .ok_or_else(|| OpenBitFunError::NotFound(format!("Model not found: {model_id}")))?;
        let provider = ai_config.providers.iter().find(|provider| {
            // Legacy flattened entries predate the provider-entity split and
            // carry no provider_id; they resolve without a provider record.
            model.provider_id.is_empty() || provider.id == model.provider_id
        });
        let resolved = match provider {
            Some(provider) => openbitfun_config_contracts::resolve_ai_model(provider, model),
            // A slim entry with a dangling provider id is not a runtime model.
            None if !model.provider_id.is_empty() => {
                return Err(OpenBitFunError::NotFound(format!(
                    "Provider not found: {}",
                    model.provider_id
                )));
            }
            None => openbitfun_config_contracts::resolve_ai_model(
                &openbitfun_config_contracts::AIProviderConfig::default(),
                model,
            ),
        };
        // Resolve default limits with the same pure conversion used by the client factory.
        // Do not create a client, resolve credentials, or contact a model provider.
        let model_config = crate::util::types::AIConfig::try_from(resolved)
            .map_err(|error| OpenBitFunError::validation(error.to_string()))?;
        let supports_images = model
            .capabilities
            .contains(&ModelCapability::ImageUnderstanding)
            || matches!(model.category, ModelCategory::Multimodal);
        let model_facts = PrimaryModelFacts::new(
            &model_id,
            &model_config.model,
            &model_config.format,
            supports_images,
        );
        let context_window = pool_window.unwrap_or(
            (model_config.context_window as usize).min(session.config.max_context_tokens),
        );
        let policy = registry
            .get_agent_tool_policy(&session.agent_type, workspace.workspace_id.as_deref())
            .await;
        vars.extend([
            (
                "enable_deferred_tool_loading".to_string(),
                ai_config.enable_deferred_tool_loading.to_string(),
            ),
            ("turn_index".to_string(), inputs.turn_index.to_string()),
        ]);
        if session.config.tool_presentation_mode.is_ptc() {
            vars.insert("tool_presentation_mode".into(), "ptc".into());
            vars.insert(
                "ptc_language".into(),
                session
                    .config
                    .tool_presentation_mode
                    .language()
                    .as_str()
                    .into(),
            );
        }

        let tool_context = tool_context_runtime::build_tool_description_context(
            &session.agent_type,
            Some(&workspace),
            workspace_services.as_ref(),
            Some(&session.session_id),
            terminal_port,
            remote_exec_port,
            Some(&model_facts),
            &vars,
            &restrictions,
        );
        let manifest = if session.config.enable_tools {
            Some(
                resolve_tool_manifest(
                    &policy.allowed_tools,
                    &policy.exposure_overrides,
                    &tool_context,
                )
                .await,
            )
        } else {
            None
        };
        let current_sections = match &manifest {
            Some(manifest) => Self::build_tool_listing_sections(manifest, &tool_context).await,
            None => ToolListingSections::default(),
        };
        let needs = manifest
            .as_ref()
            .map(|manifest| {
                RuntimeContextNeeds::from_tool_names(manifest.allowed_tool_names.iter())
            })
            .unwrap_or_default();
        let user_policy = agent.user_context_policy();
        let prompt_context = build_prompt_context_for_workspace(
            &workspace,
            workspace.workspace_id.as_deref(),
            &session.session_id,
            Some(model_config.model.clone()),
            Some(supports_images),
            current_sections.clone(),
            needs,
            &user_policy,
        )
        .await
        .ok_or_else(|| OpenBitFunError::service("Prompt context is unavailable"))?;
        let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let mut system_identity = agent.system_prompt_cache_identity(Some(&model_config.model));
        system_identity
            .scope_key
            .push_str("|genui-profile:disabled");
        let system_prompt = match inputs
            .prompt_cache
            .system_prompt
            .as_ref()
            .filter(|cached| cached.is_usable(&system_identity, inputs.cache_ttl, now))
        {
            Some(cached) => cached.text.content.clone(),
            None => agent.get_system_prompt(Some(&prompt_context)).await?,
        };
        let mut user_identity = agent.user_context_cache_identity();
        if let Some(remote) = &prompt_context.remote_execution {
            user_identity.scope_key.push_str(&format!(
                "|remote:{}",
                remote.connection_display_name.replace('|', "/")
            ));
        }
        let user_context = match inputs
            .prompt_cache
            .user_context
            .as_ref()
            .filter(|cached| cached.is_usable(&user_identity, inputs.cache_ttl, now))
        {
            Some(cached) => Some(cached.text.content.clone()),
            None => {
                let (content, complete) = Self::build_user_context_for_cache_miss(
                    Some(&workspace),
                    workspace_services.as_ref(),
                    prompt_context.clone(),
                    &user_policy,
                )
                .await;
                if !complete {
                    return Err(OpenBitFunError::service(
                        "Workspace instructions are unavailable; context estimate was not produced",
                    ));
                }
                content
            }
        };
        let baseline = inputs
            .listing_baseline
            .as_ref()
            .map(build_skill_agent_tool_listing_sections_from_snapshot);
        let (skill_listing, agent_listing) =
            skill_agent_listing_reminders(baseline.as_ref().or(Some(&current_sections)));
        let reminders = PrependedPromptReminders {
            runtime_context: PromptBuilder::new(prompt_context)
                .build_runtime_context_reminder()
                .await,
            user_context,
            skill_listing,
            agent_listing,
            deferred_tool_listing: deferred_tool_listing_reminder(
                &current_sections,
                baseline.as_ref(),
            ),
            ptc_tool_calling_guide: current_sections.ptc_tool_calling_guide.clone(),
        };
        let mut messages = inputs.messages;
        messages.retain(|message| message.role != MessageRole::System);
        messages.insert(0, Message::system(system_prompt));
        let tools = manifest
            .as_ref()
            .map(|manifest| manifest.tool_definitions.as_slice());
        Self::estimate_context_view(
            &messages,
            &reminders,
            tools,
            context_window,
            &inputs.turn_id,
            supports_images,
            options.include_contents,
            now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn estimate_context_view(
        messages: &[Message],
        reminders: &PrependedPromptReminders,
        tools: Option<&[ToolDefinition]>,
        context_window: usize,
        turn_id: &str,
        supports_images: bool,
        include_contents: bool,
        captured_at: u64,
    ) -> OpenBitFunResult<UsageContextEstimate> {
        let composition = Self::context_composition_for_send(
            messages,
            reminders,
            tools,
            context_window,
            turn_id,
            supports_images,
        );
        let contents = if include_contents {
            super::super::context_composition::context_preview_contents(
                messages.iter().filter(|message| {
                    !Self::skip_message_for_model_send(message)
                        && !Self::is_stale_interrupted_continue(message, turn_id)
                }),
                reminders,
                tools,
            )
            .map_err(|error| OpenBitFunError::parse(error.to_string()))?
        } else {
            Default::default()
        };
        Ok(UsageContextEstimate {
            captured_at,
            composition: Some(composition),
            contents,
            unavailable_reason: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_context_estimate_tracks_compression_without_request_records() {
        let system = Message::system("System instructions".into());
        let old = Message::user("Long conversation content ".repeat(10_000));
        let reminders = PrependedPromptReminders::default();
        let before = ExecutionEngine::estimate_context_view(
            &[system.clone(), old],
            &reminders,
            None,
            128_000,
            "turn-1",
            false,
            true,
            1,
        )
        .unwrap();
        let mut summary = Message::user("Compressed conversation".into());
        summary.metadata.semantic_kind = Some(MessageSemanticKind::CompressionSummary);
        let after = ExecutionEngine::estimate_context_view(
            &[system, summary],
            &reminders,
            None,
            128_000,
            "turn-1",
            false,
            true,
            2,
        )
        .unwrap();
        assert!(before.composition.as_ref().unwrap().conversation > 10_000);
        let counts = after.composition.unwrap();
        assert_eq!(counts.conversation, 0);
        assert!(counts.summarized_conversation > 0);
        assert_eq!(before.contents, after.contents);
    }

    #[test]
    fn current_context_estimate_preview_is_opt_in_and_matches_numeric_sources() {
        let mut rules = Message::user("Current rule".into());
        rules.metadata.internal_reminder_kind = Some(InternalReminderKind::ConditionalInstructions);
        let reminders = PrependedPromptReminders::default();
        let numeric = ExecutionEngine::estimate_context_view(
            std::slice::from_ref(&rules),
            &reminders,
            None,
            128_000,
            "turn-1",
            false,
            false,
            1,
        )
        .unwrap();
        assert!(numeric.contents.is_empty());
        let preview = ExecutionEngine::estimate_context_view(
            &[rules.clone()],
            &reminders,
            None,
            128_000,
            "turn-1",
            false,
            true,
            1,
        )
        .unwrap();
        assert_eq!(numeric.composition, preview.composition);
        assert_eq!(
            preview.composition.unwrap().rules,
            rules.estimate_tokens_with_reasoning(true) as u64
        );
        assert_eq!(
            preview.contents[&openbitfun_core_types::UsageContextCategory::Rules],
            "Current rule"
        );
    }
}
