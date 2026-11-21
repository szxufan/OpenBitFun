use super::ConversationCoordinator;
use crate::agentic::coordination::coordination_store::{
    BackgroundTaskRegistration, RegisteredBackgroundTask,
};
use crate::agentic::core::Session;
use crate::agentic::core::{SessionConfig, SessionContinuationPolicy};
use crate::agentic::events::{EventQueue, EventQueueConfig, EventRouter};
use crate::agentic::execution::{
    ExecutionEngine, ExecutionEngineConfig, RoundExecutor, StreamProcessor,
};
use crate::agentic::persistence::PersistenceManager;
use crate::agentic::session::{
    compression::ContextCompressor, PromptCachePolicy, SessionContextStore, SessionManager,
    SessionManagerConfig,
};
use crate::agentic::tools::registry::ToolRegistry;
use crate::agentic::tools::{ToolPipeline, ToolStateManager};
use crate::infrastructure::PathManager;
use crate::runtime_ownership::CoreRuntimeOwnership;
use crate::service::config::types::{AIConfig, AIModelConfig};
use openbitfun_agent_runtime::permission::PermissionRequestManager;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock as TokioRwLock;

#[allow(dead_code)]
pub(super) fn test_ai_config(models: Vec<AIModelConfig>) -> AIConfig {
    let mut providers = Vec::new();
    let entries = models
        .iter()
        .map(|model| openbitfun_config_contracts::split_flattened_model(model, &mut providers))
        .collect::<Vec<_>>();
    AIConfig {
        providers,
        models: entries,
        ..Default::default()
    }
}

#[allow(dead_code)]
pub(super) fn push_test_model(config: &mut AIConfig, model: AIModelConfig) {
    let entry = openbitfun_config_contracts::split_flattened_model(&model, &mut config.providers);
    config.models.push(entry);
}

pub(super) fn test_coordinator_with_registry(
    max_active_sessions: usize,
    enable_persistence: bool,
    runtime_ownership: Arc<CoreRuntimeOwnership>,
    registry: ToolRegistry,
    permission_request_manager: Option<Arc<PermissionRequestManager>>,
) -> (ConversationCoordinator, Arc<SessionManager>) {
    let event_queue = Arc::new(EventQueue::new(EventQueueConfig::default()));
    let coordination_database_file = std::env::temp_dir()
        .join(format!(
            "openbitfun-coordinator-test-{}",
            uuid::Uuid::new_v4()
        ))
        .join("coordination.sqlite");
    let session_manager = Arc::new(SessionManager::new(
        Arc::new(SessionContextStore::new()),
        Arc::new(
            PersistenceManager::new(Arc::new(PathManager::new().expect("path manager")))
                .expect("persistence manager"),
        ),
        SessionManagerConfig {
            max_active_sessions,
            session_idle_timeout: Duration::from_secs(3600),
            auto_save_interval: Duration::from_secs(300),
            enable_persistence,
            prompt_cache_policy: PromptCachePolicy::default(),
        },
    ));
    let mut tool_pipeline = ToolPipeline::new(
        Arc::new(TokioRwLock::new(registry)),
        Arc::new(ToolStateManager::new(event_queue.clone())),
        None,
    );
    if let Some(manager) = permission_request_manager {
        tool_pipeline = tool_pipeline.with_permission_request_manager(manager);
    }
    let tool_pipeline = Arc::new(tool_pipeline);
    let execution_engine = Arc::new(ExecutionEngine::new(
        Arc::new(RoundExecutor::new(
            Arc::new(StreamProcessor::new(event_queue.clone())),
            event_queue.clone(),
            tool_pipeline.clone(),
        )),
        event_queue.clone(),
        session_manager.clone(),
        Arc::new(ContextCompressor::new()),
        ExecutionEngineConfig::default(),
    ));
    let coordinator = ConversationCoordinator::new_with_coordination_database_file(
        session_manager.clone(),
        execution_engine,
        tool_pipeline,
        event_queue,
        Arc::new(EventRouter::new()),
        coordination_database_file,
        runtime_ownership,
    );
    coordinator.set_terminal_port(
        openbitfun_runtime_services::test_support::FakeRuntimeServicesProvider::terminal_port(),
    );
    coordinator.set_remote_exec_port(
        openbitfun_runtime_services::test_support::FakeRuntimeServicesProvider::remote_exec_port(),
    );

    (coordinator, session_manager)
}

pub(super) fn test_coordinator_with_config_and_ownership(
    max_active_sessions: usize,
    enable_persistence: bool,
    runtime_ownership: Arc<CoreRuntimeOwnership>,
) -> (ConversationCoordinator, Arc<SessionManager>) {
    test_coordinator_with_registry(
        max_active_sessions,
        enable_persistence,
        runtime_ownership,
        ToolRegistry::new(),
        None,
    )
}

pub(super) fn test_coordinator_with_config(
    max_active_sessions: usize,
    enable_persistence: bool,
) -> (ConversationCoordinator, Arc<SessionManager>) {
    let ownership_root = std::env::temp_dir().join(format!(
        "openbitfun-runtime-ownership-test-{}",
        uuid::Uuid::new_v4()
    ));
    test_coordinator_with_config_and_ownership(
        max_active_sessions,
        enable_persistence,
        Arc::new(CoreRuntimeOwnership::embedded_with_facts(
            ownership_root,
            "openbitfun".to_string(),
            "test",
        )),
    )
}

pub(super) fn test_coordinator_with_max_active_sessions(
    max_active_sessions: usize,
) -> (ConversationCoordinator, Arc<SessionManager>) {
    test_coordinator_with_config(max_active_sessions, false)
}

pub(super) fn test_persistent_coordinator() -> (ConversationCoordinator, Arc<SessionManager>) {
    test_coordinator_with_config(100, true)
}

pub(crate) fn test_coordinator() -> (ConversationCoordinator, Arc<SessionManager>) {
    test_coordinator_with_max_active_sessions(100)
}

pub(super) async fn register_test_background_task(
    coordinator: &ConversationCoordinator,
    parent_session_id: &str,
    parent_dialog_turn_id: &str,
    child_session_id: &str,
) -> RegisteredBackgroundTask {
    coordinator
        .background_subagent_outcomes
        .register(BackgroundTaskRegistration {
            parent_session_id: parent_session_id.to_string(),
            requested_agent_id: None,
            child_session_id: child_session_id.to_string(),
            parent_dialog_turn_id: parent_dialog_turn_id.to_string(),
            parent_tool_call_id: format!("tool-{child_session_id}"),
            child_dialog_turn_id: format!("turn-{child_session_id}"),
        })
        .await
        .expect("register background task")
}

pub(crate) async fn agent_lifecycle_child(
    coordinator: &ConversationCoordinator,
    workspace: &Path,
    parent: &str,
    policy: SessionContinuationPolicy,
) -> Session {
    let child = coordinator
        .create_hidden_subagent_session(
            None,
            "Lifecycle child".into(),
            "GeneralPurpose".into(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.to_string_lossy().into_owned()),
                continuation_policy: policy,
                ..Default::default()
            },
            Some(format!("session-{parent}")),
        )
        .await
        .expect("create child");
    coordinator
        .agent_id_for_subagent_session_with_requested_id(parent, &child.session_id, None)
        .await
        .expect("register child");
    child
}
