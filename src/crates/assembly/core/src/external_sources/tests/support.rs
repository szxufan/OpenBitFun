use super::super::*;
use openbitfun_product_domains::external_sources::{
    ExternalSourceProviderError, ExternalSourceRecord, ExternalSourceScope,
    PromptCommandAvailability, PromptCommandDefinition, PromptCommandProviderIdentity,
    PromptCommandProviderSnapshot, SourceQualifiedCommandId,
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct DelayedProvider {
    identity: PromptCommandProviderIdentity,
    source: SourceKey,
    command_name: String,
    delay: std::time::Duration,
    release: Option<Arc<StdMutex<std::sync::mpsc::Receiver<()>>>>,
    calls: Arc<AtomicUsize>,
}

impl PromptCommandSourceProvider for DelayedProvider {
    fn identity(&self) -> PromptCommandProviderIdentity {
        self.identity.clone()
    }

    fn discover(
        &self,
        context: &ExternalSourceContext,
    ) -> Result<PromptCommandProviderSnapshot, ExternalSourceProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(release) = &self.release {
            // A disconnected channel also releases the provider, so a
            // panicking test cannot strand the blocking worker.
            let _ = release
                .lock()
                .expect("provider release gate remains available")
                .recv();
        } else {
            std::thread::sleep(self.delay);
        }
        let record = ExternalSourceRecord {
            key: self.source.clone(),
            ecosystem_id: self.identity.ecosystem_id.clone(),
            display_name: self.identity.display_name.clone(),
            source_kind: "prompt_commands".to_string(),
            scope: ExternalSourceScope::UserGlobal,
            location: format!("/{}", self.command_name),
            execution_domain_id: context.execution_domain_id.clone(),
            health: openbitfun_product_domains::external_sources::ExternalSourceHealth::Available,
            content_version: "source-v1".to_string(),
            diagnostics: Vec::new(),
        };
        Ok(PromptCommandProviderSnapshot {
            provider: self.identity.clone(),
            sources: vec![record],
            commands: vec![PromptCommandDefinition {
                id: SourceQualifiedCommandId::new(self.source.clone(), self.command_name.clone())
                    .unwrap(),
                name: self.command_name.clone(),
                description: self.command_name.clone(),
                template: self.command_name.clone(),
                shell_preference: None,
                execution_target: Default::default(),
                availability: PromptCommandAvailability::Available,
                content_version: "command-v1".to_string(),
            }],
            unavailable_command_ids: Vec::new(),
            diagnostics: Vec::new(),
        })
    }

    fn expand(
        &self,
        _context: &ExternalSourceContext,
        command: &PromptCommandDefinition,
        _arguments: &str,
    ) -> Result<PromptCommandExpansion, ExternalSourceProviderError> {
        Ok(PromptCommandExpansion {
            content: command.template.clone(),
            workspace_file_references: Vec::new(),
            shell: None,
        })
    }

    fn watch_roots(
        &self,
        _context: &ExternalSourceContext,
    ) -> Vec<openbitfun_product_domains::external_sources::ExternalWatchRoot> {
        Vec::new()
    }
}

pub(super) fn delayed_provider(
    id: &str,
    delay: std::time::Duration,
    calls: Arc<AtomicUsize>,
) -> Arc<dyn PromptCommandSourceProvider> {
    Arc::new(DelayedProvider {
        identity: PromptCommandProviderIdentity::new(id, id, id).unwrap(),
        source: SourceKey::new(id, "global").unwrap(),
        command_name: id.to_string(),
        delay,
        release: None,
        calls,
    })
}

pub(super) fn blocked_provider(
    id: &str,
    calls: Arc<AtomicUsize>,
) -> (
    Arc<dyn PromptCommandSourceProvider>,
    std::sync::mpsc::Sender<()>,
) {
    let (release, wait_for_release) = std::sync::mpsc::channel();
    (
        Arc::new(DelayedProvider {
            identity: PromptCommandProviderIdentity::new(id, id, id).unwrap(),
            source: SourceKey::new(id, "global").unwrap(),
            command_name: id.to_string(),
            delay: std::time::Duration::ZERO,
            release: Some(Arc::new(StdMutex::new(wait_for_release))),
            calls,
        }),
        release,
    )
}

pub(super) fn test_service(
    providers: Vec<Arc<dyn PromptCommandSourceProvider>>,
) -> Arc<WorkspaceExternalSourceService> {
    let context = ExternalSourceContext {
        workspace_root: None,
        execution_domain_id: ExecutionDomainId::new("local-user").unwrap(),
    };
    let (updates, _) = broadcast::channel(8);
    let control_plane = Arc::new(
        ExternalSourceControlPlane::new(
            context,
            ExternalMcpRevisionKey::new([7; 32]),
            providers,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap(),
    );
    let mut snapshot = merge_tool_state(
        control_plane.commands(|coordinator| coordinator.snapshot()),
        &control_plane.tools(|coordinator| coordinator.snapshot()),
        ExternalToolProductState::default(),
    );
    snapshot.integration_policy =
        integration_policy_snapshot(&ExternalSourcesConfig::default(), None)
            .expect("built-in integration policy is valid");
    Arc::new(WorkspaceExternalSourceService {
        workspace_id: None,
        profile: ExternalSourceServiceProfile::LocalExecution,
        workspace_root: None,
        execution_domain_id: ExecutionDomainId::new(LEGACY_LOCAL_EXECUTION_DOMAIN_ID).unwrap(),
        mcp_revision_key: ExternalMcpRevisionKey::new([7; 32]),
        control_plane,
        snapshot: StdMutex::new(snapshot),
        updates,
        watch_states: tokio::sync::Mutex::new(BTreeMap::new()),
        refresh_gate: tokio::sync::Mutex::new(()),
        product_rebuild_gate: tokio::sync::Mutex::new(()),
        mcp_runtime: Arc::new(OpenBitFunExternalMcpRuntime),
        active_mcp_runtime_ids: tokio::sync::Mutex::new(BTreeSet::new()),
        initial_refresh_completed: AtomicBool::new(false),
        automatic_discovery_active: AtomicBool::new(false),
        background_refresh_scheduled: AtomicBool::new(false),
        initial_refresh_gate: tokio::sync::Mutex::new(()),
        keepalive_started: AtomicBool::new(false),
        last_access_epoch_seconds: AtomicU64::new(epoch_seconds()),
        watcher: Arc::new(FileWatchService::new(FileWatcherConfig::default())),
        tool_decision_gate_waiting: tokio::sync::Notify::new(),
        tool_decision_gate_acquired: tokio::sync::Notify::new(),
        subagent_expiry_schedule: AtomicU64::new(0),
    })
}

#[derive(Default)]
pub(super) struct CountingExternalMcpRuntime {
    pub(super) calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ExternalMcpRuntimePort for CountingExternalMcpRuntime {
    async fn install(
        &self,
        _candidate: &crate::external_mcp::ActiveExternalMcpCandidate,
        _prepared: openbitfun_product_domains::external_sources::PreparedExternalMcpServer,
        _workspace_key: &str,
    ) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn retire(&self, _runtime_id: &str) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn status(&self, _runtime_id: &str) -> Result<ExternalMcpRuntimeStatus, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ExternalMcpRuntimeStatus::Active)
    }

    async fn replace_workspace_route(
        &self,
        _workspace_key: &str,
        _active_external_server_ids: BTreeSet<String>,
        _suppressed_native_server_ids: BTreeSet<String>,
    ) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
