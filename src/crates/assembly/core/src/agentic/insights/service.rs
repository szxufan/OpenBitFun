use crate::agentic::insights::cancellation::{self, Registration};
use crate::agentic::insights::collector::InsightsCollector;
use crate::agentic::insights::facet_cache;
use crate::agentic::insights::html::generate_html;
use crate::agentic::insights::prompt_context::{
    aggregate_stats_json_for_prompt, friction_block, summaries_block, user_instructions_block,
};
use crate::agentic::insights::types::*;
use crate::infrastructure::ai::get_global_ai_client_factory;
use crate::infrastructure::ai::AIClient;
use crate::infrastructure::events::{emit_global_event, BackendEvent};
use crate::infrastructure::get_path_manager_arc;
use crate::service::config::get_global_config_service;
use crate::service::config::AppConfig;
use crate::service::i18n::LocaleId;
use crate::util::errors::{OpenBitFunError, OpenBitFunResult};
use crate::util::types::{GeminiResponse, GeminiUsage, Message, ToolDefinition};
use futures::{stream, StreamExt};
use log::{debug, info, warn};
use openbitfun_agent_content::insights::{
    AREAS as AREAS_PROMPT_TEMPLATE, AT_A_GLANCE as AT_A_GLANCE_PROMPT_TEMPLATE,
    FACET_EXTRACTION as FACET_PROMPT_TEMPLATE, FRICTION as FRICTION_PROMPT_TEMPLATE,
    FUN_ENDING as FUN_ENDING_PROMPT_TEMPLATE, HORIZON as HORIZON_PROMPT_TEMPLATE,
    INTERACTION_STYLE as INTERACTION_STYLE_PROMPT_TEMPLATE,
    SUGGESTIONS as SUGGESTIONS_PROMPT_TEMPLATE, WINS as WINS_PROMPT_TEMPLATE,
};
use openbitfun_services_core::json_store::JsonFileStore;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;

const MAX_CONCURRENT_FACET_EXTRACTIONS: usize = 5;
const ANALYSIS_SECTION_COUNT: u32 = 8;

struct TrackedAIClient {
    client: Arc<AIClient>,
    model_id: String,
    usage: GenerationUsageTracker,
}

impl TrackedAIClient {
    fn new(client: Arc<AIClient>, model_id: String, usage: GenerationUsageTracker) -> Self {
        Self {
            client,
            model_id,
            usage,
        }
    }

    fn model_identity(&self) -> [&str; 4] {
        let config = &self.client.config;
        [
            &self.model_id,
            &config.format,
            &config.request_url,
            &config.model,
        ]
    }

    async fn send_message(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> anyhow::Result<GeminiResponse> {
        self.usage.record_model_call();
        let response = self.client.send_message(messages, tools).await?;
        self.usage.record_reported_usage(response.usage.as_ref());
        Ok(response)
    }
}

#[derive(Clone, Default)]
struct GenerationUsageTracker {
    usage: Arc<Mutex<InsightsGenerationUsage>>,
}

impl GenerationUsageTracker {
    fn with_usage<R>(&self, update: impl FnOnce(&mut InsightsGenerationUsage) -> R) -> R {
        let mut usage = self
            .usage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        update(&mut usage)
    }

    fn record_model_call(&self) {
        self.with_usage(|usage| {
            usage.model_calls = usage.model_calls.saturating_add(1);
        });
    }

    fn record_reported_usage(&self, reported: Option<&GeminiUsage>) {
        let Some(reported) = reported else {
            return;
        };

        self.with_usage(|usage| {
            usage.input_tokens = usage
                .input_tokens
                .saturating_add(reported.prompt_token_count as u64);
            usage.output_tokens = usage
                .output_tokens
                .saturating_add(reported.candidates_token_count as u64);
            usage.total_tokens = usage
                .total_tokens
                .saturating_add(reported.total_token_count as u64);
            usage.reasoning_tokens = usage
                .reasoning_tokens
                .saturating_add(reported.reasoning_token_count.unwrap_or_default() as u64);
            usage.cached_input_tokens = usage
                .cached_input_tokens
                .saturating_add(reported.cached_content_token_count.unwrap_or_default() as u64);
            usage.cache_creation_input_tokens = usage
                .cache_creation_input_tokens
                .saturating_add(reported.cache_creation_token_count.unwrap_or_default() as u64);
            usage.reported_model_calls = usage.reported_model_calls.saturating_add(1);
        });
    }

    fn snapshot(&self) -> InsightsGenerationUsage {
        self.with_usage(|usage| usage.clone())
    }
}

pub struct InsightsService;

fn normalize_insights_model_selector(model_selector: Option<&str>) -> &str {
    match model_selector.map(str::trim).unwrap_or_default() {
        // Upgrade-only input compatibility for UI payloads created before the
        // Auto selector was removed. New callers send `primary` directly.
        "" | "auto" | "default" => "primary",
        selector => selector,
    }
}

impl InsightsService {
    fn validate_days(days: u32) -> OpenBitFunResult<()> {
        if (1..=3650).contains(&days) {
            Ok(())
        } else {
            Err(OpenBitFunError::validation(
                "Insights range must be between 1 and 3650 days",
            ))
        }
    }

    async fn get_user_language() -> LocaleId {
        let Ok(config_service) = get_global_config_service().await else {
            return LocaleId::EnUS;
        };
        config_service
            .get_config::<AppConfig>(Some("app"))
            .await
            .ok()
            .and_then(|config| LocaleId::from_str(&config.language))
            .unwrap_or(LocaleId::EnUS)
    }

    fn build_language_instruction(locale: LocaleId) -> String {
        let json_rule = concat!(
            "\n\nCRITICAL JSON RULE: Inside JSON string values you MUST escape every literal double-quote as \\\".",
            " Do NOT place unescaped \" characters inside string values.",
            " For example, write \"he said \\\"hello\\\"\" instead of \"he said \"hello\"\".",
        );
        format!(
            "\n\nIMPORTANT: All descriptive text, summaries, suggestions, and narrative content in your response MUST be written in {}. Keep JSON keys and enum values in English.{}",
            locale.model_language_name(), json_rule
        )
    }

    pub async fn generate(
        days: u32,
        model_selector: Option<String>,
        generation_id: Option<String>,
    ) -> OpenBitFunResult<InsightsReport> {
        Self::validate_days(days)?;
        let generation = cancellation::register(generation_id).await;
        let pipeline = cancellation::PIPELINE.lock().await;
        let result = cancellation::run_until_cancelled(
            &generation.token,
            Box::pin(Self::build_report(
                days,
                model_selector.as_deref(),
                &generation,
            )),
        )
        .await;

        let result = match result {
            Ok(report) => {
                if cancellation::claim_completion(generation.id).await {
                    let result = Self::save_report(report).await;
                    if result.is_ok() {
                        Self::emit_progress(&generation, "Complete!", "complete", 0, 0).await;
                        info!("Insights report generated");
                    }
                    result
                } else {
                    Err(OpenBitFunError::service("Insights generation cancelled"))
                }
            }
            Err(error) => Err(error),
        };
        cancellation::unregister(generation.id).await;
        drop(pipeline);
        result
    }

    pub async fn cancel(generation_id: Option<&str>) -> Result<(), String> {
        cancellation::cancel(generation_id).await
    }

    async fn build_report(
        days: u32,
        model_selector: Option<&str>,
        generation: &Registration,
    ) -> OpenBitFunResult<InsightsReport> {
        let user_lang = Self::get_user_language().await;
        let lang_instruction = Self::build_language_instruction(user_lang);
        debug!("Insights generation using language: {}", user_lang);

        // Stage 1: Data Collection
        Self::emit_progress(
            generation,
            "Collecting session data...",
            "data_collection",
            0,
            0,
        )
        .await;
        let (base_stats, transcripts) = InsightsCollector::collect(days).await?;

        if transcripts.is_empty() {
            return Err(OpenBitFunError::service(
                "No sessions found in the specified time range",
            ));
        }

        info!(
            "Collected {} sessions, {} messages",
            transcripts.len(),
            base_stats.total_messages
        );

        // Stage 2: Parallel Facet Extraction
        let ai_factory = get_global_ai_client_factory().await.map_err(|e| {
            OpenBitFunError::service(format!("Failed to get AI client factory: {}", e))
        })?;
        let selected_model = normalize_insights_model_selector(model_selector);
        let model_id = ai_factory
            .resolve_model_id(selected_model)
            .await
            .map_err(|e| {
                OpenBitFunError::service(format!("Failed to resolve selected insights model: {e}"))
            })?;
        let ai_client = ai_factory.get_client_by_id(&model_id).await.map_err(|e| {
            OpenBitFunError::service(format!("Failed to load selected insights model: {e}"))
        })?;

        let generation_models = vec![ai_client.config.model.clone()];

        let usage_tracker = GenerationUsageTracker::default();
        let ai_client = TrackedAIClient::new(ai_client, model_id, usage_tracker.clone());

        let facets =
            Self::extract_facets_adaptive(&ai_client, &transcripts, &lang_instruction, generation)
                .await?;

        info!("Extracted facets for {} sessions", facets.len());

        // Stage 3: Aggregation (Rust-side, no AI)
        Self::emit_progress(generation, "Aggregating analysis...", "aggregation", 0, 0).await;
        let aggregate = InsightsCollector::aggregate(&base_stats, &facets);

        // Stage 4a: Parallel analysis — 7 independent tasks
        Self::emit_progress(generation, "Analyzing patterns...", "analysis", 0, 0).await;

        let (suggestions, areas, wins_friction, interaction, horizon, fun_ending) =
            Self::generate_analysis_parallel(&ai_client, &aggregate, &lang_instruction, generation)
                .await;
        let mut failed_sections = 0;
        let suggestions =
            section_or_default(suggestions, &mut failed_sections, default_suggestions);
        let areas = section_or_default(areas, &mut failed_sections, Vec::new);
        let wins_friction = WinsFrictionResult::from_sections(wins_friction, &mut failed_sections);
        let interaction = section_or_default(
            interaction,
            &mut failed_sections,
            InteractionStyleResult::default,
        );
        let horizon = section_or_default(horizon, &mut failed_sections, HorizonResult::default);
        let fun_ending = section_or_default(fun_ending, &mut failed_sections, || None);

        // Stage 4b: Synthesis — at_a_glance depends on 4a results
        Self::emit_progress(generation, "Writing summary...", "synthesis", 0, 0).await;

        let at_a_glance = Self::generate_synthesis(
            &ai_client,
            &aggregate,
            &suggestions,
            &areas,
            &wins_friction,
            &interaction,
            &lang_instruction,
        )
        .await;
        let at_a_glance = section_or_default(at_a_glance, &mut failed_sections, AtAGlance::default);
        ensure_report_has_analysis(facets.len(), failed_sections)?;

        // Stage 5: Assembly
        Self::emit_progress(generation, "Assembling report...", "assembly", 0, 0).await;
        let report = Self::assemble_report(
            base_stats,
            aggregate,
            suggestions,
            areas,
            wins_friction,
            interaction,
            at_a_glance,
            horizon,
            fun_ending,
            usage_tracker.snapshot(),
            generation_models,
            failed_sections,
            user_lang,
        );

        Ok(report)
    }

    // ============ Stage 2: Facet Extraction ============

    async fn extract_facets_adaptive(
        ai_client: &TrackedAIClient,
        transcripts: &[SessionTranscript],
        lang_instruction: &str,
        generation: &Registration,
    ) -> OpenBitFunResult<Vec<SessionFacet>> {
        let total = transcripts.len();
        let counter = AtomicUsize::new(0);
        let rate_limited = AtomicBool::new(false);
        let counter = &counter;
        let rate_limited = &rate_limited;
        let mut pending = stream::iter(0..total)
            .map(move |index| async move {
                if rate_limited.load(Ordering::Relaxed) {
                    return (index, None);
                }
                let current = counter.fetch_add(1, Ordering::Relaxed) + 1;
                Self::emit_progress(
                    generation,
                    &format!("Analyzing session {current}/{total}..."),
                    "facet_extraction",
                    current,
                    total,
                )
                .await;
                let result =
                    Self::extract_single_facet(ai_client, &transcripts[index], lang_instruction)
                        .await;
                if result.as_ref().is_err_and(is_rate_limit_error) {
                    rate_limited.store(true, Ordering::Relaxed);
                }
                (index, Some(result))
            })
            .buffer_unordered(MAX_CONCURRENT_FACET_EXTRACTIONS);

        let mut facets = Vec::new();
        let mut retry_indices = Vec::new();
        while let Some((index, result)) = pending.next().await {
            match result {
                Some(Ok(facet)) => facets.push((index, facet)),
                Some(Err(error)) if !is_rate_limit_error(&error) => {
                    warn!("Facet extraction failed for session {index}: {error}");
                }
                _ => retry_indices.push(index),
            }
        }
        if !retry_indices.is_empty() {
            retry_indices.sort_unstable();
            let retry_count = retry_indices.len();
            Self::emit_progress(
                generation,
                &format!("Rate limited. Retrying {retry_count} sessions sequentially..."),
                "facet_retry",
                0,
                retry_count,
            )
            .await;
            tokio::time::sleep(Duration::from_secs(3)).await;

            for (attempt, index) in retry_indices.into_iter().enumerate() {
                Self::emit_progress(
                    generation,
                    &format!("Retrying session {}/{retry_count}...", attempt + 1),
                    "facet_retry",
                    attempt + 1,
                    retry_count,
                )
                .await;
                match Self::extract_single_facet(ai_client, &transcripts[index], lang_instruction)
                    .await
                {
                    Ok(facet) => facets.push((index, facet)),
                    Err(error) => warn!("Sequential retry failed for session {index}: {error}"),
                }
                if attempt + 1 < retry_count {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }

        ensure_report_has_analysis(facets.len(), 0)?;
        facets.sort_unstable_by_key(|(index, _)| *index);
        Ok(facets.into_iter().map(|(_, facet)| facet).collect())
    }

    async fn extract_single_facet(
        ai_client: &TrackedAIClient,
        transcript: &SessionTranscript,
        lang_instruction: &str,
    ) -> OpenBitFunResult<SessionFacet> {
        let session_info = format!(
            "Session: {}\nAgent: {}\nName: {}\nDate: {}\nDuration: {} min\n\n{}",
            transcript.session_id,
            transcript.agent_type,
            transcript.session_name,
            transcript.created_at,
            transcript.duration_minutes,
            transcript.transcript
        );

        let prompt = format!(
            "{}{}",
            FACET_PROMPT_TEMPLATE.replace("{session_transcript}", &session_info),
            lang_instruction
        );
        let model_identity = ai_client.model_identity();
        if let Ok(Some(cached)) =
            facet_cache::try_load_cached_facet(transcript, &model_identity, &prompt).await
        {
            return Ok(cached);
        }
        let messages = vec![Message::user(prompt.clone())];

        let response = ai_client
            .send_message(messages, None)
            .await
            .map_err(|e| OpenBitFunError::service(format!("AI call failed: {}", e)))?;

        let json_str = extract_json_from_response(&response.text)?;
        let value: Value = serde_json::from_str(&json_str).map_err(|e| {
            OpenBitFunError::Deserialization(format!("Failed to parse facet JSON: {}", e))
        })?;

        let brief_summary = value["brief_summary"]
            .as_str()
            .filter(|summary| !summary.trim().is_empty())
            .ok_or_else(|| OpenBitFunError::service("Session analysis has no summary"))?;
        let facet = SessionFacet {
            session_id: transcript.session_id.clone(),
            underlying_goal: value["underlying_goal"].as_str().unwrap_or("").to_string(),
            goal_categories: parse_string_u32_map(&value["goal_categories"]),
            outcome: value["outcome"]
                .as_str()
                .unwrap_or("unclear_from_transcript")
                .to_string(),
            user_satisfaction_counts: parse_string_u32_map(&value["user_satisfaction_counts"]),
            claude_helpfulness: value["claude_helpfulness"]
                .as_str()
                .unwrap_or("moderately_helpful")
                .to_string(),
            session_type: value["session_type"]
                .as_str()
                .unwrap_or("single_task")
                .to_string(),
            friction_counts: parse_string_u32_map(&value["friction_counts"]),
            friction_detail: value["friction_detail"].as_str().unwrap_or("").to_string(),
            primary_success: value["primary_success"].as_str().unwrap_or("").to_string(),
            brief_summary: brief_summary.to_owned(),
            languages_used: value["languages_used"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            user_instructions: value["user_instructions"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        };

        let _ = facet_cache::save_cached_facet(transcript, &facet, &model_identity, &prompt).await;

        Ok(facet)
    }

    // ============ Stage 4a: Parallel Analysis ============

    async fn generate_analysis_parallel(
        ai_client: &TrackedAIClient,
        aggregate: &InsightsAggregate,
        lang_instruction: &str,
        generation: &Registration,
    ) -> (
        OpenBitFunResult<InsightsSuggestions>,
        OpenBitFunResult<Vec<ProjectArea>>,
        (
            OpenBitFunResult<WinsResult>,
            OpenBitFunResult<FrictionResult>,
        ),
        OpenBitFunResult<InteractionStyleResult>,
        OpenBitFunResult<HorizonResult>,
        OpenBitFunResult<Option<FunEnding>>,
    ) {
        let aggregate_json = aggregate_stats_json_for_prompt(aggregate);
        let summaries = summaries_block(aggregate);
        let friction_details = friction_block(aggregate);
        let semaphore = Semaphore::new(3);

        let (suggestions, areas, wins, friction, interaction, horizon, ending) = tokio::join!(
            Self::resolve_with_retry(generation, &semaphore, "Suggestions", || {
                Self::generate_suggestions(ai_client, aggregate, lang_instruction)
            }),
            Self::resolve_with_retry(generation, &semaphore, "Areas", || {
                Self::identify_areas(ai_client, aggregate, lang_instruction)
            }),
            Self::resolve_with_retry(generation, &semaphore, "Wins", || {
                Self::analyze_wins(ai_client, &aggregate_json, &summaries, lang_instruction)
            }),
            Self::resolve_with_retry(generation, &semaphore, "Friction", || {
                Self::analyze_friction(
                    ai_client,
                    &aggregate_json,
                    &summaries,
                    &friction_details,
                    lang_instruction,
                )
            }),
            Self::resolve_with_retry(generation, &semaphore, "Interaction Style", || {
                Self::analyze_interaction_style(
                    ai_client,
                    &aggregate_json,
                    &summaries,
                    lang_instruction,
                )
            }),
            Self::resolve_with_retry(generation, &semaphore, "Horizon", || {
                Self::generate_horizon(
                    ai_client,
                    &aggregate_json,
                    &summaries,
                    &friction_details,
                    lang_instruction,
                )
            }),
            Self::resolve_with_retry(generation, &semaphore, "Fun Ending", || {
                Self::generate_fun_ending(ai_client, &aggregate_json, &summaries, lang_instruction)
            }),
        );
        (
            suggestions,
            areas,
            (wins, friction),
            interaction,
            horizon,
            ending,
        )
    }

    async fn resolve_with_retry<T, F>(
        generation: &Registration,
        semaphore: &Semaphore,
        label: &str,
        call: impl Fn() -> F,
    ) -> OpenBitFunResult<T>
    where
        F: std::future::Future<Output = OpenBitFunResult<T>>,
    {
        let attempt = || async {
            let _permit = semaphore
                .acquire()
                .await
                .map_err(|error| OpenBitFunError::service(error.to_string()))?;
            Box::pin(call()).await
        };
        let result = attempt().await;
        if let Err(error) = &result {
            if is_retryable_error(error) {
                warn!("{label} failed (retryable): {error}");
                Self::emit_progress(
                    generation,
                    &format!("Retrying {}...", label.to_lowercase()),
                    "analysis_retry",
                    0,
                    0,
                )
                .await;
                tokio::time::sleep(Duration::from_secs(3)).await;
                return attempt().await;
            }
        }
        result
    }

    // ============ Stage 4b: Synthesis ============

    async fn generate_synthesis(
        ai_client: &TrackedAIClient,
        aggregate: &InsightsAggregate,
        suggestions: &InsightsSuggestions,
        areas: &[ProjectArea],
        wins_friction: &WinsFrictionResult,
        interaction: &InteractionStyleResult,
        lang_instruction: &str,
    ) -> OpenBitFunResult<AtAGlance> {
        let aggregate_json = aggregate_stats_json_for_prompt(aggregate);

        let areas_text = areas
            .iter()
            .map(|a| format!("- {}: {}", a.name, a.description))
            .collect::<Vec<_>>()
            .join("\n");
        let suggestions_text =
            serde_json::to_string_pretty(suggestions).unwrap_or_else(|_| "{}".to_string());
        let wins_friction_text =
            serde_json::to_string_pretty(wins_friction).unwrap_or_else(|_| "{}".to_string());
        let interaction_text =
            serde_json::to_string_pretty(interaction).unwrap_or_else(|_| "{}".to_string());

        Self::generate_at_a_glance(
            ai_client,
            &aggregate_json,
            &areas_text,
            &suggestions_text,
            &wins_friction_text,
            &interaction_text,
            lang_instruction,
        )
        .await
    }

    // ============ Individual Analysis Methods ============

    async fn generate_suggestions(
        ai_client: &TrackedAIClient,
        aggregate: &InsightsAggregate,
        lang_instruction: &str,
    ) -> OpenBitFunResult<InsightsSuggestions> {
        let aggregate_json = aggregate_stats_json_for_prompt(aggregate);
        let summaries = summaries_block(aggregate);
        let friction_details = friction_block(aggregate);
        let user_instructions = user_instructions_block(aggregate);

        let prompt = format!(
            "{}{}",
            SUGGESTIONS_PROMPT_TEMPLATE
                .replace("{aggregate_json}", &aggregate_json)
                .replace("{summaries}", &summaries)
                .replace("{friction_details}", &friction_details)
                .replace("{user_instructions}", &user_instructions),
            lang_instruction
        );

        let messages = vec![Message::user(prompt)];
        let response = ai_client
            .send_message(messages, None)
            .await
            .map_err(|e| OpenBitFunError::service(format!("Suggestions AI call failed: {}", e)))?;

        info!(
            "Suggestions response: len={}, finish={:?}",
            response.text.len(),
            response.finish_reason
        );
        debug!("Suggestions text: {}", safe_truncate(&response.text, 300));

        let json_str = extract_json_from_response(&response.text)?;
        let value: Value = serde_json::from_str(&json_str).map_err(|e| {
            OpenBitFunError::Deserialization(format!(
                "Failed to parse suggestions JSON: {}. Raw: {}",
                e,
                safe_truncate(&json_str, 500)
            ))
        })?;
        require_response_arrays(
            &value,
            &[
                "openbitfun_md_additions",
                "features_to_try",
                "usage_patterns",
            ],
        )?;

        debug!(
            "Suggestions parsed: md_additions={}, features={}, patterns={}",
            value["openbitfun_md_additions"]
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0),
            value["features_to_try"]
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0),
            value["usage_patterns"]
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0),
        );

        Ok(InsightsSuggestions {
            openbitfun_md_additions: value["openbitfun_md_additions"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| {
                            Some(MdAddition {
                                section: v["section"].as_str()?.to_string(),
                                content: v["content"].as_str()?.to_string(),
                                rationale: v["rationale"]
                                    .as_str()
                                    .or(v["why"].as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            features_to_try: value["features_to_try"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| {
                            Some(FeatureRecommendation {
                                feature: v["feature"].as_str()?.to_string(),
                                description: v["description"]
                                    .as_str()
                                    .or(v["one_liner"].as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                example_usage: v["example_usage"]
                                    .as_str()
                                    .or(v["example_code"].as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                benefit: v["benefit"]
                                    .as_str()
                                    .or(v["why_for_you"].as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            usage_patterns: value["usage_patterns"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|v| UsagePattern {
                            pattern: v["pattern"]
                                .as_str()
                                .or(v["title"].as_str())
                                .unwrap_or("")
                                .to_string(),
                            description: v["description"]
                                .as_str()
                                .or(v["suggestion"].as_str())
                                .unwrap_or("")
                                .to_string(),
                            detail: v["detail"].as_str().unwrap_or("").to_string(),
                            suggested_prompt: v["suggested_prompt"]
                                .as_str()
                                .or(v["copyable_prompt"].as_str())
                                .unwrap_or("")
                                .to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    async fn identify_areas(
        ai_client: &TrackedAIClient,
        aggregate: &InsightsAggregate,
        lang_instruction: &str,
    ) -> OpenBitFunResult<Vec<ProjectArea>> {
        let aggregate_json = aggregate_stats_json_for_prompt(aggregate);
        let summaries = summaries_block(aggregate);

        let prompt = format!(
            "{}{}",
            AREAS_PROMPT_TEMPLATE
                .replace("{aggregate_json}", &aggregate_json)
                .replace("{summaries}", &summaries),
            lang_instruction
        );

        let messages = vec![Message::user(prompt)];
        let response = ai_client
            .send_message(messages, None)
            .await
            .map_err(|e| OpenBitFunError::service(format!("Areas AI call failed: {}", e)))?;

        info!(
            "Areas response: len={}, finish={:?}",
            response.text.len(),
            response.finish_reason
        );
        debug!("Areas text: {}", safe_truncate(&response.text, 300));

        let json_str = extract_json_from_response(&response.text)?;
        let value: Value = serde_json::from_str(&json_str).map_err(|e| {
            OpenBitFunError::Deserialization(format!("Failed to parse areas JSON: {}", e))
        })?;
        require_response_arrays(&value, &["areas"])?;

        Ok(value["areas"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        Some(ProjectArea {
                            name: v["name"].as_str()?.to_string(),
                            session_count: v["session_count"].as_u64().unwrap_or(0) as u32,
                            description: v["description"].as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn analyze_wins(
        ai_client: &TrackedAIClient,
        aggregate_json: &str,
        summaries: &str,
        lang_instruction: &str,
    ) -> OpenBitFunResult<WinsResult> {
        let prompt = format!(
            "{}{}",
            WINS_PROMPT_TEMPLATE
                .replace("{aggregate_json}", aggregate_json)
                .replace("{summaries}", summaries),
            lang_instruction
        );

        let messages = vec![Message::user(prompt)];
        let response = ai_client
            .send_message(messages, None)
            .await
            .map_err(|e| OpenBitFunError::service(format!("Wins AI call failed: {}", e)))?;

        info!(
            "Wins response: len={}, finish={:?}",
            response.text.len(),
            response.finish_reason
        );
        debug!("Wins text: {}", safe_truncate(&response.text, 300));

        let json_str = extract_json_from_response(&response.text)?;
        let value: Value = serde_json::from_str(&json_str).map_err(|e| {
            OpenBitFunError::Deserialization(format!("Failed to parse wins JSON: {}", e))
        })?;
        require_response_arrays(&value, &["impressive_workflows"])?;

        Ok(WinsResult {
            intro: value["intro"].as_str().unwrap_or("").to_string(),
            big_wins: value["impressive_workflows"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| {
                            Some(BigWin {
                                title: v["title"].as_str()?.to_string(),
                                description: v["description"].as_str()?.to_string(),
                                impact: v["impact"].as_str().unwrap_or("").to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    async fn analyze_friction(
        ai_client: &TrackedAIClient,
        aggregate_json: &str,
        summaries: &str,
        friction_details: &str,
        lang_instruction: &str,
    ) -> OpenBitFunResult<FrictionResult> {
        let prompt = format!(
            "{}{}",
            FRICTION_PROMPT_TEMPLATE
                .replace("{aggregate_json}", aggregate_json)
                .replace("{summaries}", summaries)
                .replace("{friction_details}", friction_details),
            lang_instruction
        );

        let messages = vec![Message::user(prompt)];
        let response = ai_client
            .send_message(messages, None)
            .await
            .map_err(|e| OpenBitFunError::service(format!("Friction AI call failed: {}", e)))?;

        info!(
            "Friction response: len={}, finish={:?}",
            response.text.len(),
            response.finish_reason
        );
        debug!("Friction text: {}", safe_truncate(&response.text, 300));

        let json_str = extract_json_from_response(&response.text)?;
        let value: Value = serde_json::from_str(&json_str).map_err(|e| {
            OpenBitFunError::Deserialization(format!("Failed to parse friction JSON: {}", e))
        })?;
        require_response_arrays(&value, &["friction_categories"])?;

        Ok(FrictionResult {
            intro: value["intro"].as_str().unwrap_or("").to_string(),
            friction_categories: value["friction_categories"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| {
                            Some(FrictionCategory {
                                category: v["category"].as_str()?.to_string(),
                                count: v["count"].as_u64().unwrap_or(0) as u32,
                                description: v["description"].as_str()?.to_string(),
                                examples: v["examples"]
                                    .as_array()
                                    .map(|a| {
                                        a.iter()
                                            .filter_map(|e| e.as_str().map(String::from))
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                                suggestion: v["suggestion"].as_str().unwrap_or("").to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    async fn analyze_interaction_style(
        ai_client: &TrackedAIClient,
        aggregate_json: &str,
        summaries: &str,
        lang_instruction: &str,
    ) -> OpenBitFunResult<InteractionStyleResult> {
        let prompt = format!(
            "{}{}",
            INTERACTION_STYLE_PROMPT_TEMPLATE
                .replace("{aggregate_json}", aggregate_json)
                .replace("{summaries}", summaries),
            lang_instruction
        );

        let messages = vec![Message::user(prompt)];
        let response = ai_client.send_message(messages, None).await.map_err(|e| {
            OpenBitFunError::service(format!("Interaction Style AI call failed: {}", e))
        })?;

        info!(
            "Interaction Style response: len={}, finish={:?}",
            response.text.len(),
            response.finish_reason
        );
        debug!(
            "Interaction Style text: {}",
            safe_truncate(&response.text, 300)
        );

        let json_str = extract_json_from_response(&response.text)?;
        let value: Value = serde_json::from_str(&json_str).map_err(|e| {
            OpenBitFunError::Deserialization(format!(
                "Failed to parse interaction style JSON: {}",
                e
            ))
        })?;
        if value["narrative"]
            .as_str()
            .is_none_or(|text| text.trim().is_empty())
        {
            return Err(OpenBitFunError::service(
                "Interaction style has no narrative",
            ));
        }

        Ok(InteractionStyleResult {
            narrative: value["narrative"].as_str().unwrap_or("").to_string(),
            key_patterns: value["key_patterns"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    async fn generate_at_a_glance(
        ai_client: &TrackedAIClient,
        aggregate_json: &str,
        areas_text: &str,
        suggestions_text: &str,
        wins_friction_text: &str,
        interaction_text: &str,
        lang_instruction: &str,
    ) -> OpenBitFunResult<AtAGlance> {
        let prompt = format!(
            "{}{}",
            AT_A_GLANCE_PROMPT_TEMPLATE
                .replace("{aggregate_json}", aggregate_json)
                .replace("{areas}", areas_text)
                .replace("{suggestions}", suggestions_text)
                .replace("{wins_and_friction}", wins_friction_text)
                .replace("{interaction_style}", interaction_text),
            lang_instruction
        );

        let messages = vec![Message::user(prompt)];
        let response = ai_client
            .send_message(messages, None)
            .await
            .map_err(|e| OpenBitFunError::service(format!("At a Glance AI call failed: {}", e)))?;

        info!(
            "At a Glance response: len={}, finish={:?}",
            response.text.len(),
            response.finish_reason
        );
        debug!("At a Glance text: {}", safe_truncate(&response.text, 300));

        let json_str = extract_json_from_response(&response.text)?;
        let value: Value = serde_json::from_str(&json_str).map_err(|e| {
            OpenBitFunError::Deserialization(format!("Failed to parse at-a-glance JSON: {}", e))
        })?;
        if json_value_to_string(&value["whats_working"])
            .trim()
            .is_empty()
        {
            return Err(OpenBitFunError::service("Report summary has no analysis"));
        }

        let looking_ahead = {
            let v = json_value_to_string(&value["looking_ahead"]);
            if v.is_empty() {
                json_value_to_string(&value["ambitious_workflows"])
            } else {
                v
            }
        };

        Ok(AtAGlance {
            whats_working: json_value_to_string(&value["whats_working"]),
            whats_hindering: json_value_to_string(&value["whats_hindering"]),
            quick_wins: json_value_to_string(&value["quick_wins"]),
            looking_ahead,
        })
    }

    async fn generate_horizon(
        ai_client: &TrackedAIClient,
        aggregate_json: &str,
        summaries: &str,
        friction_details: &str,
        lang_instruction: &str,
    ) -> OpenBitFunResult<HorizonResult> {
        let prompt = format!(
            "{}{}",
            HORIZON_PROMPT_TEMPLATE
                .replace("{aggregate_json}", aggregate_json)
                .replace("{summaries}", summaries)
                .replace("{friction_details}", friction_details),
            lang_instruction
        );

        let messages = vec![Message::user(prompt)];
        let response = ai_client
            .send_message(messages, None)
            .await
            .map_err(|e| OpenBitFunError::service(format!("Horizon AI call failed: {}", e)))?;

        info!(
            "Horizon response: len={}, finish={:?}",
            response.text.len(),
            response.finish_reason
        );
        debug!("Horizon text: {}", safe_truncate(&response.text, 300));

        let json_str = extract_json_from_response(&response.text)?;
        let value: Value = serde_json::from_str(&json_str).map_err(|e| {
            OpenBitFunError::Deserialization(format!("Failed to parse horizon JSON: {}", e))
        })?;
        require_response_arrays(&value, &["opportunities"])?;

        Ok(HorizonResult {
            intro: value["intro"].as_str().unwrap_or("").to_string(),
            opportunities: value["opportunities"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| {
                            Some(HorizonWorkflow {
                                title: v["title"].as_str()?.to_string(),
                                whats_possible: v["whats_possible"].as_str()?.to_string(),
                                how_to_try: v["how_to_try"].as_str().unwrap_or("").to_string(),
                                copyable_prompt: v["copyable_prompt"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    async fn generate_fun_ending(
        ai_client: &TrackedAIClient,
        aggregate_json: &str,
        summaries: &str,
        lang_instruction: &str,
    ) -> OpenBitFunResult<Option<FunEnding>> {
        let prompt = format!(
            "{}{}",
            FUN_ENDING_PROMPT_TEMPLATE
                .replace("{aggregate_json}", aggregate_json)
                .replace("{summaries}", summaries),
            lang_instruction
        );

        let messages = vec![Message::user(prompt)];
        let response = ai_client
            .send_message(messages, None)
            .await
            .map_err(|e| OpenBitFunError::service(format!("Fun Ending AI call failed: {}", e)))?;

        info!(
            "Fun Ending response: len={}, finish={:?}",
            response.text.len(),
            response.finish_reason
        );
        debug!("Fun Ending text: {}", safe_truncate(&response.text, 300));

        let json_str = extract_json_from_response(&response.text)?;
        let value: Value = serde_json::from_str(&json_str).map_err(|e| {
            OpenBitFunError::Deserialization(format!("Failed to parse fun ending JSON: {}", e))
        })?;
        if value["headline"]
            .as_str()
            .or(value["title"].as_str())
            .is_none_or(|text| text.trim().is_empty())
        {
            return Err(OpenBitFunError::service("Report ending has no headline"));
        }

        Ok(Some(FunEnding {
            headline: value["headline"]
                .as_str()
                .or(value["title"].as_str())
                .unwrap_or("")
                .to_string(),
            detail: value["detail"]
                .as_str()
                .or(value["message"].as_str())
                .unwrap_or("")
                .to_string(),
        }))
    }

    // ============ Stage 5: Assembly ============

    #[allow(clippy::too_many_arguments)]
    fn assemble_report(
        base_stats: BaseStats,
        aggregate: InsightsAggregate,
        suggestions: InsightsSuggestions,
        areas: Vec<ProjectArea>,
        wins_friction: WinsFrictionResult,
        interaction: InteractionStyleResult,
        at_a_glance: AtAGlance,
        horizon: HorizonResult,
        fun_ending: Option<FunEnding>,
        generation_usage: InsightsGenerationUsage,
        generation_models: Vec<String>,
        failed_sections: u32,
        locale: LocaleId,
    ) -> InsightsReport {
        let days_covered =
            if !aggregate.date_range.start.is_empty() && !aggregate.date_range.end.is_empty() {
                let parse = |s: &str| -> Option<chrono::DateTime<chrono::Utc>> {
                    chrono::DateTime::parse_from_rfc3339(s)
                        .ok()
                        .map(|d| d.with_timezone(&chrono::Utc))
                };
                match (
                    parse(&aggregate.date_range.start),
                    parse(&aggregate.date_range.end),
                ) {
                    (Some(start), Some(end)) => {
                        end.date_naive()
                            .signed_duration_since(start.date_naive())
                            .num_days()
                            .unsigned_abs() as u32
                            + 1
                    }
                    _ => 1,
                }
                .max(1)
            } else {
                1
            };

        InsightsReport {
            generated_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            locale: Some(locale.as_str().to_owned()),
            date_range: aggregate.date_range.clone(),
            total_sessions: aggregate.sessions,
            analyzed_sessions: aggregate.analyzed,
            failed_sections,
            total_messages: aggregate.messages,
            days_covered,
            session_usage: aggregate.session_usage.clone(),
            generation_usage,
            generation_models,
            stats: InsightsStats {
                total_hours: aggregate.hours,
                msgs_per_day: aggregate.msgs_per_day,
                top_tools: aggregate.top_tools.clone(),
                top_goals: aggregate.top_goals.clone(),
                outcomes: aggregate.outcomes.clone(),
                satisfaction: aggregate.satisfaction.clone(),
                session_types: aggregate.session_types.clone(),
                languages: aggregate.languages.clone(),
                hour_counts: aggregate.hour_counts.clone(),
                activity_by_utc_minute: base_stats.activity_by_utc_minute,
                agent_types: aggregate.agent_types.clone(),
                response_time_buckets: aggregate.response_time_buckets.clone(),
                median_response_time_secs: aggregate.median_response_time_secs,
                avg_response_time_secs: aggregate.avg_response_time_secs,
                friction: aggregate.friction.clone(),
                success: aggregate.success.clone(),
                tool_errors: aggregate.tool_errors.clone(),
                total_lines_added: aggregate.total_lines_added,
                total_lines_removed: aggregate.total_lines_removed,
                total_files_modified: aggregate.total_files_modified,
            },
            at_a_glance,
            interaction_style: InteractionStyle {
                narrative: interaction.narrative,
                key_patterns: interaction.key_patterns,
            },
            project_areas: areas,
            wins_intro: wins_friction.wins_intro,
            big_wins: wins_friction.big_wins,
            friction_intro: wins_friction.friction_intro,
            friction_categories: wins_friction.friction_categories,
            suggestions,
            horizon_intro: horizon.intro,
            on_the_horizon: horizon.opportunities,
            fun_ending,
            html_report_path: None,
        }
    }

    // ============ Save / Load / Utility ============

    async fn save_report(mut report: InsightsReport) -> OpenBitFunResult<InsightsReport> {
        let path_manager = get_path_manager_arc();
        let usage_dir = path_manager.user_data_dir().join("usage-data");
        tokio::fs::create_dir_all(&usage_dir)
            .await
            .map_err(|e| OpenBitFunError::io(format!("Failed to create usage-data dir: {}", e)))?;

        let timestamp = report.generated_at;

        let html_path = usage_dir.join(format!("insights-{}.html", timestamp));
        Self::write_report_html(&report, &html_path, report_locale(&report, LocaleId::EnUS))
            .await?;

        report.html_report_path = Some(html_path.to_string_lossy().to_string());

        let json_path = usage_dir.join(format!("insights-{}.json", timestamp));
        JsonFileStore
            .write_atomic_strict(&json_path, &report)
            .await
            .map_err(|e| OpenBitFunError::io(format!("Failed to write report JSON: {e}")))?;

        info!(
            "Report saved: json={}, html={}",
            json_path.display(),
            html_path.display()
        );

        Self::cleanup_old_reports(&usage_dir, 5).await;

        Ok(report)
    }

    async fn write_report_html(
        report: &InsightsReport,
        path: &std::path::Path,
        locale: LocaleId,
    ) -> OpenBitFunResult<()> {
        JsonFileStore
            .write_text_atomic_strict(path, &generate_html(report, locale))
            .await
            .map_err(|e| OpenBitFunError::io(format!("Failed to write HTML report: {e}")))
    }

    async fn refresh_report_html(
        report: &mut InsightsReport,
        json_path: &std::path::Path,
        current_locale: LocaleId,
    ) {
        let path = json_path.with_extension("html");
        let locale = report_locale(report, current_locale);
        report.html_report_path = match Self::write_report_html(report, &path, locale).await {
            Ok(()) => Some(path.to_string_lossy().into_owned()),
            Err(error) => {
                warn!("Could not regenerate safe HTML for saved insights report: {error}");
                None
            }
        };
    }

    async fn cleanup_old_reports(usage_dir: &std::path::Path, keep: usize) {
        let mut entries = match tokio::fs::read_dir(usage_dir).await {
            Ok(dir) => dir,
            Err(_) => return,
        };

        let mut json_files: Vec<std::path::PathBuf> = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("insights-") && name.ends_with(".json") {
                json_files.push(entry.path());
            }
        }

        json_files.sort();
        json_files.reverse();

        for old in json_files.into_iter().skip(keep) {
            let _ = tokio::fs::remove_file(&old).await;
            let html = old.with_extension("html");
            let _ = tokio::fs::remove_file(&html).await;
        }
    }

    pub async fn has_data(days: u32) -> OpenBitFunResult<bool> {
        Self::validate_days(days)?;
        let (_, transcripts) = InsightsCollector::collect(days).await?;
        Ok(!transcripts.is_empty())
    }

    pub async fn load_report(path: &str) -> OpenBitFunResult<InsightsReport> {
        let path_manager = get_path_manager_arc();
        let usage_dir = path_manager.user_data_dir().join("usage-data");
        let requested_path = std::path::PathBuf::from(path);
        let valid_name = requested_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("insights-") && name.ends_with(".json"));
        if !valid_name {
            return Err(OpenBitFunError::validation("Invalid insights report path"));
        }

        let canonical_usage_dir = tokio::fs::canonicalize(&usage_dir).await.map_err(|e| {
            OpenBitFunError::io(format!("Failed to resolve insights directory: {}", e))
        })?;
        let canonical_path = tokio::fs::canonicalize(&requested_path)
            .await
            .map_err(|e| {
                OpenBitFunError::io(format!("Failed to resolve insights report: {}", e))
            })?;
        if !canonical_path.starts_with(&canonical_usage_dir) {
            return Err(OpenBitFunError::validation(
                "Insights report path is outside the report directory",
            ));
        }

        let json_str = tokio::fs::read_to_string(&canonical_path)
            .await
            .map_err(|e| OpenBitFunError::io(format!("Failed to read report file: {}", e)))?;
        let mut report: InsightsReport = serde_json::from_str(&json_str).map_err(|e| {
            OpenBitFunError::Deserialization(format!("Failed to parse report: {}", e))
        })?;
        Self::refresh_report_html(
            &mut report,
            &canonical_path,
            Self::get_user_language().await,
        )
        .await;
        Ok(report)
    }

    pub async fn load_latest_reports() -> OpenBitFunResult<Vec<InsightsReportMeta>> {
        let path_manager = get_path_manager_arc();
        let usage_dir = path_manager.user_data_dir().join("usage-data");

        if !usage_dir.exists() {
            return Ok(vec![]);
        }

        let mut entries = tokio::fs::read_dir(&usage_dir)
            .await
            .map_err(|e| OpenBitFunError::io(format!("Failed to read usage-data dir: {}", e)))?;

        let mut json_files: Vec<std::path::PathBuf> = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("insights-") && name.ends_with(".json") {
                json_files.push(entry.path());
            }
        }

        json_files.sort();
        json_files.reverse();

        let mut reports = Vec::new();
        for json_path in json_files.iter().take(10) {
            match tokio::fs::read_to_string(json_path).await {
                Ok(json_str) => match serde_json::from_str::<InsightsReport>(&json_str) {
                    Ok(report) => {
                        let top_goals: Vec<String> = report
                            .stats
                            .top_goals
                            .iter()
                            .take(3)
                            .map(|(name, _)| name.clone())
                            .collect();
                        let mut lang_entries: Vec<_> = report.stats.languages.iter().collect();
                        lang_entries.sort_by(|(_, a), (_, b)| b.cmp(a));
                        let languages: Vec<String> = lang_entries
                            .iter()
                            .take(3)
                            .map(|(name, _)| name.to_string())
                            .collect();

                        reports.push(InsightsReportMeta {
                            generated_at: report.generated_at,
                            total_sessions: report.total_sessions,
                            analyzed_sessions: report.analyzed_sessions,
                            failed_sections: report.failed_sections,
                            date_range: report.date_range,
                            path: json_path.to_string_lossy().to_string(),
                            total_messages: report.total_messages,
                            days_covered: report.days_covered,
                            total_hours: report.stats.total_hours,
                            top_goals,
                            languages,
                            session_usage: report.session_usage,
                            generation_usage: report.generation_usage,
                            generation_models: report.generation_models,
                        });
                    }
                    Err(e) => {
                        warn!("Failed to parse report {}: {}", json_path.display(), e);
                    }
                },
                Err(e) => {
                    warn!("Failed to read report {}: {}", json_path.display(), e);
                }
            }
        }

        Ok(reports)
    }

    async fn emit_progress(
        generation: &Registration,
        message: &str,
        stage: &str,
        current: usize,
        total: usize,
    ) {
        if generation.token.is_cancelled() {
            return;
        }
        let payload = serde_json::json!({
            "generationId": generation.client_id,
            "message": message,
            "stage": stage,
            "current": current,
            "total": total,
        });
        if let Err(e) = emit_global_event(BackendEvent::Custom {
            event_name: "insights-progress".to_string(),
            payload,
        })
        .await
        {
            debug!("Failed to emit progress event: {}", e);
        }
    }
}

// ============ Intermediate result types (internal to service) ============

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct WinsFrictionResult {
    #[serde(default)]
    wins_intro: String,
    big_wins: Vec<BigWin>,
    #[serde(default)]
    friction_intro: String,
    friction_categories: Vec<FrictionCategory>,
}

impl WinsFrictionResult {
    fn from_sections(
        (wins, friction): (
            OpenBitFunResult<WinsResult>,
            OpenBitFunResult<FrictionResult>,
        ),
        failed_sections: &mut u32,
    ) -> Self {
        let wins = section_or_default(wins, failed_sections, WinsResult::default);
        let friction = section_or_default(friction, failed_sections, FrictionResult::default);
        Self {
            wins_intro: wins.intro,
            big_wins: wins.big_wins,
            friction_intro: friction.intro,
            friction_categories: friction.friction_categories,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct WinsResult {
    intro: String,
    big_wins: Vec<BigWin>,
}

impl WinsResult {
    fn default() -> Self {
        Self {
            intro: String::new(),
            big_wins: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct FrictionResult {
    intro: String,
    friction_categories: Vec<FrictionCategory>,
}

impl FrictionResult {
    fn default() -> Self {
        Self {
            intro: String::new(),
            friction_categories: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct InteractionStyleResult {
    narrative: String,
    key_patterns: Vec<String>,
}

impl InteractionStyleResult {
    fn default() -> Self {
        Self {
            narrative: String::new(),
            key_patterns: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct HorizonResult {
    intro: String,
    opportunities: Vec<HorizonWorkflow>,
}

impl HorizonResult {
    fn default() -> Self {
        Self {
            intro: String::new(),
            opportunities: Vec::new(),
        }
    }
}

impl AtAGlance {
    fn default() -> Self {
        Self {
            whats_working: "Analysis in progress...".to_string(),
            whats_hindering: String::new(),
            quick_wins: String::new(),
            looking_ahead: String::new(),
        }
    }
}

// ============ Helper functions ============

fn is_rate_limit_error(e: &OpenBitFunError) -> bool {
    let msg = e.to_string().to_lowercase();
    msg.contains("429")
        || msg.contains("rate limit")
        || msg.contains("too many requests")
        || msg.contains("rate_limit")
}

fn is_retryable_error(e: &OpenBitFunError) -> bool {
    if is_rate_limit_error(e) {
        return true;
    }
    let msg = e.to_string().to_lowercase();
    msg.contains("cannot extract json")
        || msg.contains("sse stream closed")
        || msg.contains("stream closed before")
        || msg.contains("connection reset")
}

fn report_locale(report: &InsightsReport, current_locale: LocaleId) -> LocaleId {
    if let Some(locale) = report.locale.as_deref().and_then(LocaleId::from_str) {
        return locale;
    }

    // Legacy reports did not persist a locale. Use narrative prose only, since
    // chart categories and technical names can be English in Chinese reports.
    let prose = [
        &report.at_a_glance.whats_working,
        &report.at_a_glance.whats_hindering,
        &report.at_a_glance.quick_wins,
        &report.at_a_glance.looking_ahead,
        &report.interaction_style.narrative,
        &report.wins_intro,
        &report.friction_intro,
        &report.horizon_intro,
    ];
    let (han, latin) =
        prose
            .iter()
            .flat_map(|text| text.chars())
            .fold((0usize, 0usize), |(han, latin), ch| {
                (
                    han + usize::from(
                        matches!(ch, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}'),
                    ),
                    latin + usize::from(ch.is_ascii_alphabetic()),
                )
            });

    if han >= 8 && han * 3 >= latin {
        if current_locale == LocaleId::ZhTW {
            LocaleId::ZhTW
        } else {
            LocaleId::ZhCN
        }
    } else if latin >= 24 && latin > han * 8 {
        LocaleId::EnUS
    } else {
        current_locale
    }
}

fn ensure_report_has_analysis(
    analyzed_sessions: usize,
    failed_sections: u32,
) -> OpenBitFunResult<()> {
    if analyzed_sessions == 0 {
        return Err(OpenBitFunError::service(
            "The selected model could not analyze any sessions",
        ));
    }
    if failed_sections == ANALYSIS_SECTION_COUNT {
        return Err(OpenBitFunError::service(
            "The selected model could not generate any report sections",
        ));
    }
    Ok(())
}

fn section_or_default<T>(
    result: OpenBitFunResult<T>,
    failed_sections: &mut u32,
    default: impl FnOnce() -> T,
) -> T {
    result.unwrap_or_else(|error| {
        warn!("Insights report section failed: {error}");
        *failed_sections += 1;
        default()
    })
}

fn require_response_arrays(value: &Value, names: &[&str]) -> OpenBitFunResult<()> {
    for name in names {
        if !value[*name].is_array() {
            return Err(OpenBitFunError::service(format!(
                "Model response is missing an array for {name}"
            )));
        }
    }
    Ok(())
}

fn default_suggestions() -> InsightsSuggestions {
    InsightsSuggestions {
        openbitfun_md_additions: Vec::new(),
        features_to_try: Vec::new(),
        usage_patterns: Vec::new(),
    }
}

fn parse_string_u32_map(value: &Value) -> std::collections::HashMap<String, u32> {
    let mut map = std::collections::HashMap::new();
    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            if let Some(n) = v.as_u64() {
                map.insert(k.clone(), n as u32);
            } else if let Some(n) = v.as_f64() {
                map.insert(k.clone(), n as u32);
            }
        }
    }
    map
}

fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn extract_json_from_response(response: &str) -> OpenBitFunResult<String> {
    crate::util::extract_json_from_ai_response(response)
        .ok_or_else(|| OpenBitFunError::service("Cannot extract JSON from AI response"))
}

/// Extract a string from a JSON value that may be a plain string or a nested object.
/// When the value is an object, concatenate all string values with spaces.
fn json_value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Object(map) => map
            .values()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_can_run_on_a_host_worker() {
        fn assert_send<T: Send>(_: T) {}
        assert_send(InsightsService::generate(30, None, None));
    }

    #[test]
    fn all_failed_analysis_cannot_be_saved_as_a_success() {
        assert!(ensure_report_has_analysis(0, 0).is_err());
        let mut failed_sections = 0;
        for _ in 0..ANALYSIS_SECTION_COUNT {
            section_or_default(
                Err(OpenBitFunError::service("Fake provider failure")),
                &mut failed_sections,
                || (),
            );
        }
        assert!(ensure_report_has_analysis(2, failed_sections).is_err());
        assert!(ensure_report_has_analysis(2, failed_sections - 1).is_ok());
        assert!(require_response_arrays(&serde_json::json!({}), &["areas"]).is_err());
        assert!(require_response_arrays(&serde_json::json!({"areas": []}), &["areas"]).is_ok());
    }

    fn report_fixture() -> InsightsReport {
        let stats = BaseStats {
            total_sessions: 2,
            ..BaseStats::default()
        };
        let aggregate = InsightsCollector::aggregate(&stats, &[]);
        InsightsService::assemble_report(
            stats,
            aggregate,
            default_suggestions(),
            vec![],
            WinsFrictionResult::from_sections(
                (Ok(WinsResult::default()), Ok(FrictionResult::default())),
                &mut 0,
            ),
            InteractionStyleResult::default(),
            AtAGlance::default(),
            HorizonResult::default(),
            None,
            InsightsGenerationUsage::default(),
            vec![],
            1,
            LocaleId::EnUS,
        )
    }

    #[tokio::test]
    async fn legacy_report_round_trips_and_gets_a_safe_export_on_open() {
        let mut legacy = serde_json::to_value(report_fixture()).unwrap();
        legacy.as_object_mut().unwrap().remove("failed_sections");
        legacy.as_object_mut().unwrap().remove("locale");
        legacy["at_a_glance"]["whats_working"] =
            serde_json::json!("The focused regression tests verify the fix.");
        legacy["stats"]["hour_counts"] = serde_json::json!({"18": 2});
        assert!(legacy["stats"].get("activity_by_utc_minute").is_none());
        let mut report: InsightsReport = serde_json::from_value(legacy).unwrap();
        assert_eq!(report.failed_sections, 0);
        assert_eq!(report.locale, None);
        assert_eq!(report.stats.hour_counts[&18], 2);
        let round_trip: InsightsReport =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert_eq!(round_trip.stats.hour_counts, report.stats.hour_counts);

        let dir = tempfile::tempdir().unwrap();
        let json_path = dir.path().join("insights-123.json");
        let html_path = json_path.with_extension("html");
        tokio::fs::write(&html_path, "old export").await.unwrap();
        report.html_report_path = Some("untrusted-legacy-path.html".to_owned());
        InsightsService::refresh_report_html(&mut report, &json_path, LocaleId::ZhCN).await;
        assert_eq!(report.html_report_path.as_deref(), html_path.to_str());
        let html = tokio::fs::read_to_string(html_path).await.unwrap();
        assert!(html.contains(r#"<html lang="en-US">"#));
        assert!(html.contains("Evening (18-24)"));
        assert!(!html.contains("id=\"tz-selector\""));
        assert!(!html.contains("old export"));
    }

    #[tokio::test]
    async fn saved_report_language_survives_an_application_language_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("insights-123.json");
        let mut report = report_fixture();
        report.locale = Some("zh-TW".to_owned());
        report.at_a_glance.quick_wins = "建議先加入有針對性的回歸測試。".to_owned();
        let persisted = serde_json::to_vec(&report).unwrap();
        let mut reopened: InsightsReport = serde_json::from_slice(&persisted).unwrap();

        InsightsService::refresh_report_html(&mut reopened, &path, LocaleId::EnUS).await;
        let html = tokio::fs::read_to_string(path.with_extension("html"))
            .await
            .unwrap();
        assert!(html.contains(r#"<html lang="zh-TW">"#));
        assert!(html.contains("概覽"));
        assert!(html.contains(&report.at_a_glance.quick_wins));
        assert_eq!(reopened.locale.as_deref(), Some("zh-TW"));
    }

    #[test]
    fn legacy_chinese_narrative_survives_an_english_application_locale() {
        let mut report = report_fixture();
        report.locale = None;
        report.at_a_glance.whats_working =
            "为导入功能添加回归测试，确保 CSV 中的空行和引号都能正常处理。".to_owned();
        assert_eq!(report_locale(&report, LocaleId::EnUS), LocaleId::ZhCN);
    }

    #[test]
    fn prompt_explicitly_requests_the_report_language_for_each_locale() {
        for locale in LocaleId::all() {
            let prompt = InsightsService::build_language_instruction(locale);
            assert!(prompt.contains(&format!("written in {}", locale.model_language_name())));
        }
    }

    #[test]
    fn partial_report_exposes_failed_sections_and_utc_buckets() {
        let mut report = report_fixture();
        report.analyzed_sessions = 1;
        report.stats.activity_by_utc_minute.insert(29_750_999, 2);
        let html = generate_html(&report, LocaleId::EnUS);
        assert!(html.contains("Partial report. 1 report sections failed."));
        assert!(html.contains("data-minutes=\"{&quot;29750999&quot;:2}\""));
        let persisted: InsightsReport =
            serde_json::from_value(serde_json::to_value(&report).unwrap()).unwrap();
        assert_eq!(persisted.failed_sections, 1);
        assert_eq!(persisted.analyzed_sessions, 1);
        assert_eq!(
            persisted.stats.activity_by_utc_minute,
            report.stats.activity_by_utc_minute
        );
    }

    #[test]
    fn retired_auto_insights_input_is_canonicalized_to_primary() {
        assert_eq!(normalize_insights_model_selector(Some(" auto ")), "primary");
        assert_eq!(normalize_insights_model_selector(None), "primary");
        assert_eq!(
            normalize_insights_model_selector(Some("configured-model")),
            "configured-model"
        );
    }

    #[test]
    fn generation_usage_tracker_aggregates_reported_calls_and_preserves_coverage() {
        let tracker = GenerationUsageTracker::default();
        let reported = GeminiUsage {
            prompt_token_count: 120,
            candidates_token_count: 30,
            total_token_count: 150,
            reasoning_token_count: Some(12),
            cached_content_token_count: Some(80),
            cache_creation_token_count: Some(10),
        };

        tracker.record_model_call();
        tracker.record_reported_usage(Some(&reported));
        tracker.record_model_call();
        tracker.record_reported_usage(None);

        assert_eq!(
            tracker.snapshot(),
            InsightsGenerationUsage {
                input_tokens: 120,
                output_tokens: 30,
                total_tokens: 150,
                reasoning_tokens: 12,
                cached_input_tokens: 80,
                cache_creation_input_tokens: 10,
                model_calls: 2,
                reported_model_calls: 1,
            }
        );
    }
}
