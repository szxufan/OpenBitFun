use super::super::{SessionManager, SessionManagerConfig, TEST_MODEL_RESOLUTION_AI_CONFIG};
use crate::agentic::persistence::PersistenceManager;
use crate::agentic::session::{
    PersistedEvidenceLedgerFile, PromptCachePolicy, SessionContextStore,
};
use crate::infrastructure::ai::reasoning_catalog::{
    project_model_reasoning_catalog as project_test_model_reasoning_catalog,
    reasoning_preset_runtime_fingerprint as test_reasoning_preset_runtime_fingerprint,
    resolve_default_reasoning_preset as resolve_test_default_reasoning_preset,
    resolve_reasoning_preset as resolve_test_reasoning_preset,
};
use crate::infrastructure::PathManager;
use crate::service::config::types::{
    model_runtime_binding_fingerprint as service_model_runtime_binding_fingerprint,
    AIConfig as ServiceAIConfig, AIModelConfig as ServiceAIModelConfig,
};
use openbitfun_core_types::{
    ReasoningCatalogBinding, ReasoningConfig, ReasoningPreset, ReasoningPresetAction,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

pub(super) struct TestWorkspace {
    path: PathBuf,
    workspace_id: String,
}

impl TestWorkspace {
    /// Creates the directory and registers it as a local workspace record
    /// in the shared fixture catalog. Sessions only exist inside registered
    /// workspaces, so a path-only `SessionConfig` naming this directory
    /// resolves to that record exactly like a folder a host has opened.
    /// The path is canonical so IO projections written back from the
    /// record compare equal on hosts with symlinked temp roots.
    pub(super) fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "openbitfun-session-restore-test-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).expect("test workspace should be created");
        let path = dunce::canonicalize(&path).expect("test workspace should canonicalize");
        let record =
            crate::service::workspace::legacy_compat::register_local_fixture_blocking(&path);
        Self {
            path,
            workspace_id: record.id,
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub(super) fn path_manager(&self) -> Arc<PathManager> {
        Arc::new(PathManager::with_user_root_for_tests(
            self.path.join("user-root"),
        ))
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub(super) fn test_manager(persistence_manager: Arc<PersistenceManager>) -> SessionManager {
    SessionManager::new(
        Arc::new(SessionContextStore::new()),
        persistence_manager,
        SessionManagerConfig {
            max_active_sessions: 100,
            session_idle_timeout: Duration::from_secs(3600),
            auto_save_interval: Duration::from_secs(300),
            enable_persistence: true,
            prompt_cache_policy: PromptCachePolicy::default(),
        },
    )
}

pub(super) async fn reopen_interrupted_turn_for_test(
    manager: &SessionManager,
    session_id: &str,
    turn_id: &str,
    expected_generation: u32,
) -> crate::util::errors::OpenBitFunResult<super::super::InterruptedTurnRecoveryPlan> {
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            test_ai_config(vec![configured_reasoning_model("model-original")]),
            manager.reopen_interrupted_dialog_turn(session_id, turn_id, expected_generation),
        )
        .await
}

pub(super) fn interrupted_turn_test_model_fingerprint() -> String {
    service_model_runtime_binding_fingerprint(&configured_reasoning_model("model-original"))
}

pub(super) fn interrupted_turn_test_reasoning_fingerprint(selected_preset: Option<&str>) -> String {
    let model = configured_reasoning_model("model-original");
    let projection = project_test_model_reasoning_catalog(&model, None);
    let preset = selected_preset
        .and_then(|preset_id| resolve_test_reasoning_preset(&projection, preset_id))
        .or_else(|| {
            selected_preset
                .is_none()
                .then(|| resolve_test_default_reasoning_preset(&projection))
                .flatten()
        });
    test_reasoning_preset_runtime_fingerprint(preset)
}

pub(super) fn test_manager_with_config(
    persistence_manager: Arc<PersistenceManager>,
    config: SessionManagerConfig,
) -> SessionManager {
    SessionManager::new(
        Arc::new(SessionContextStore::new()),
        persistence_manager,
        config,
    )
}

pub(super) fn test_path_manager() -> Arc<PathManager> {
    let root = std::env::temp_dir().join(format!(
        "openbitfun-session-manager-test-{}",
        Uuid::new_v4()
    ));
    Arc::new(PathManager::with_user_root_for_tests(
        root.join("user-root"),
    ))
}

pub(super) fn in_memory_test_manager() -> SessionManager {
    let persistence_manager =
        Arc::new(PersistenceManager::new(test_path_manager()).expect("persistence manager"));
    SessionManager::new(
        Arc::new(SessionContextStore::new()),
        persistence_manager,
        SessionManagerConfig {
            max_active_sessions: 100,
            session_idle_timeout: Duration::from_secs(3600),
            auto_save_interval: Duration::from_secs(300),
            enable_persistence: false,
            prompt_cache_policy: PromptCachePolicy::default(),
        },
    )
}

pub(super) fn test_ai_config(models: Vec<ServiceAIModelConfig>) -> ServiceAIConfig {
    let mut providers = Vec::new();
    let entries = models
        .iter()
        .map(|model| openbitfun_config_contracts::split_flattened_model(model, &mut providers))
        .collect::<Vec<_>>();
    ServiceAIConfig {
        providers,
        models: entries,
        ..Default::default()
    }
}

pub(super) fn test_model(id: &str, context_window: u32) -> ServiceAIModelConfig {
    ServiceAIModelConfig {
        id: id.to_string(),
        name: id.to_string(),
        model_name: id.to_string(),
        enabled: true,
        context_window: Some(context_window),
        ..Default::default()
    }
}

pub(super) fn configured_reasoning_model(id: &str) -> ServiceAIModelConfig {
    ServiceAIModelConfig {
        id: id.to_string(),
        name: id.to_string(),
        provider: "openai".to_string(),
        base_url: "https://example.test/v1".to_string(),
        model_name: id.to_string(),
        enabled: true,
        reasoning: Some(ReasoningConfig {
            catalog: ReasoningCatalogBinding::Disabled,
            presets: vec![ReasoningPreset {
                id: "high".to_string(),
                actions: vec![ReasoningPresetAction::Effort {
                    value: "high".to_string(),
                }],
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub(super) fn evidence_event_ids(ledger_path: &std::path::Path) -> Vec<String> {
    let stored: PersistedEvidenceLedgerFile =
        serde_json::from_slice(&std::fs::read(ledger_path).expect("ledger sidecar should exist"))
            .expect("ledger sidecar should deserialize");
    stored
        .events
        .iter()
        .map(|event| event.event_id.clone())
        .collect()
}
