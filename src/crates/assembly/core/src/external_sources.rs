//! Product composition and lifecycle service for external AI application sources.
//!
//! Concrete ecosystem providers are selected only in this assembly module. The
//! catalog and product surfaces remain provider- and ecosystem-neutral.

pub use crate::instruction_sources::{instruction_source_catalog, InstructionSourceCatalog};

pub use openbitfun_product_domains::external_integration_policy::{
    EffectiveExternalIntegrationPolicy, ExternalIntegrationAccess, ExternalIntegrationMode,
    ExternalIntegrationPolicyMutation, ExternalIntegrationPolicyOperation,
    ExternalIntegrationPolicyScope, ExternalIntegrationPolicySnapshot,
    ExternalIntegrationPolicyStatus,
};
pub use openbitfun_product_domains::external_source_control::{
    ExternalCapabilityKindV1, ExternalSourceControlActionV1, ExternalSourceControlRequestV1,
    ExternalSourceControlSnapshotV1, ExternalSourceRuntimeState, ExternalSourceSurfaceSnapshotV1,
    EXTERNAL_SOURCE_CONTROL_SCHEMA_V1,
};
use openbitfun_product_domains::external_sources::native_prompt_command_group_fingerprint;
pub use openbitfun_product_domains::external_sources::ExternalSourceDiscoverySnapshotV1;
pub use openbitfun_product_domains::external_sources::{
    native_prompt_command_conflict_key, prompt_command_conflict_key, EcosystemId,
    ExpandedPromptCommand, ExternalIntegrationCapabilityId, ExternalMcpActivationState,
    ExternalMcpApprovalRequest, ExternalMcpCatalogEntry, ExternalMcpConflict,
    ExternalMcpTransportKind, ExternalSourceAssetKind, ExternalSourceCatalogEntry,
    ExternalSourceCatalogSnapshot, ExternalSourceDiagnostic, ExternalSourceDiagnosticSeverity,
    ExternalSourceHostCapabilities, ExternalSourceLifecycleState, ExternalSourceOperationError,
    ExternalSourceOperationErrorCode, ExternalSourceOperationResult, ExternalSourcePublicSnapshot,
    ExternalToolActivationState, ExternalToolApprovalRequest, ExternalToolCapability,
    ExternalToolCatalogEntry, ExternalToolConflict, ExternalToolConflictCandidateKind,
    ExternalToolRuntimeKind, NativePromptCommandConflictProjection,
    NativePromptCommandConflictSnapshot, NativePromptCommandDescriptor,
    NativePromptCommandReconfirmationProjection, PromptCommandAvailability,
    PromptCommandCatalogEntry, PromptCommandDefinition, PromptCommandExecutionTarget,
    PromptCommandInvocationOutcome, PromptCommandShellReviewDecision, PromptCommandShellReviewMode,
    PromptCommandShellReviewPlan, SourceKey,
};
pub use openbitfun_product_domains::external_subagents::{
    ExternalSubagentActivationState, ExternalSubagentCompatibilityState, ExternalSubagentConflict,
    ExternalSubagentConflictCandidate, ExternalSubagentModelBindingGroup,
    ExternalSubagentModelBindingMethod, ExternalSubagentModelBindingOption,
    ExternalSubagentModelBindingTarget, ExternalSubagentModelProfileRequest,
    ExternalSubagentModelRequest, ExternalSubagentSummary,
};

use crate::agentic::workspace::workspace_route_key;
use crate::external_mcp::{
    reconcile_external_mcp_catalog, ExternalMcpDecision, ExternalMcpDecisions,
    ExternalMcpProductState, ExternalMcpRuntimePort, ExternalMcpRuntimeStatus, NativeMcpCandidate,
    OpenBitFunExternalMcpRuntime, EXTERNAL_MCP_RUNTIME_FAILED,
    EXTERNAL_MCP_RUNTIME_PREPARATION_FAILED, EXTERNAL_MCP_RUNTIME_STATUS_UNAVAILABLE,
};
use crate::external_subagents::{
    project_external_subagents_read_only, reconcile_external_subagents, ExternalSubagentDecisions,
    ExternalSubagentProductState, DISABLED_SUBAGENT_CONFLICT_CHOICE,
};
use crate::external_tools::{
    begin_external_tool_workspace_recovery, external_tool_workspace_requires_recovery,
    invalidate_external_tool_runtime_availability, merge_tool_state,
    project_external_tools_read_only, reconcile_external_tools, release_external_tool_workspace,
    reset_external_tool_workspace_recovery_budget, ExternalToolDecisions, ExternalToolProductState,
    TOOL_CONFLICT_RESELECTION_REQUIRED, UNRESOLVED_TOOL_CONFLICT_CHOICE,
};
use crate::service::config::{subscribe_config_updates, ConfigUpdateEvent};
use dashmap::{mapref::entry::Entry, DashMap};
use futures::future::join_all;
use openbitfun_claude_code_adapter::{
    ClaudeCodeCommandProvider, ClaudeCodeMcpProvider, ClaudeCodeSubagentProvider,
};
use openbitfun_codex_adapter::{CodexMcpProvider, CodexSubagentProvider};

use openbitfun_dsh_adapter::DshMcpProvider;
use openbitfun_external_sources::{
    DeferredDiscovery, ExternalMcpDiscoveryResult, ExternalSourceControlPlane,
    ExternalSourceCoordinator, ExternalSourceDiscoveryResult, ExternalSubagentDiscoveryResult,
    ExternalToolDiscoveryResult, ExternalWorkspaceReferenceDiscoveryResult,
};
use openbitfun_opencode_adapter::{
    OpenCodeCommandProvider, OpenCodeMcpProvider, OpenCodeSkillRootProvider,
    OpenCodeSubagentProvider, OpenCodeToolProvider, OpenCodeWorkspaceReferenceProvider,
};
#[cfg(test)]
use openbitfun_opencode_adapter::{
    OpenCodeCommandProviderOptions, OpenCodeMcpProviderOptions, OpenCodeSkillRootProviderOptions,
    OpenCodeSubagentProviderOptions,
};
use openbitfun_product_domains::external_integration_policy::{
    automatic_discovery_enabled, external_integration_policy_snapshot,
    incompatible_external_integration_policy_snapshot, ExternalIntegrationCapabilityDescriptor,
    ExternalIntegrationEcosystemDescriptor, ExternalIntegrationPolicyDocument,
    ExternalIntegrationPolicySettings, EXTERNAL_INTEGRATION_POLICY_SCHEMA_MAJOR,
};
use openbitfun_product_domains::external_sources::{
    ExecutionDomainId, ExternalMcpRevisionKey, ExternalMcpSourceProvider, ExternalSourceContext,
    ExternalSourceScope, ExternalToolSourceProvider, PromptCommandConflict, PromptCommandExpansion,
    PromptCommandShellInvocation, PromptCommandShellPreference, PromptCommandSourceProvider,
};
use openbitfun_product_domains::external_subagents::ExternalSubagentSourceProvider;
use openbitfun_product_domains::workspace_references::{
    ExternalWorkspaceReferenceSourceProvider, WorkspaceReferenceCatalogEntry,
    WorkspaceReferenceOrigin, WorkspaceReferenceSnapshot,
};
use openbitfun_services_core::json_store::JsonFileStore;
use openbitfun_services_core::workspace_text::read_workspace_relative_text_bounded;
use openbitfun_services_integrations::file_watch::{FileWatchService, FileWatcherConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard, OnceLock, Weak};
use terminal_core::exec::{ExecControlAction, ExecControlOrigin, ExecControlRequest};
use terminal_core::{
    resolve_local_exec_shell_without_probe, ExecProcessManager, LocalExecCommandRequest,
    ShellDetector, ShellType,
};
use tokio::sync::broadcast;
use tool_runtime::exec_command::{
    exec_command_argv_for_isolated_shell, exec_command_noninteractive_env, ExecCommandShellKind,
};

pub use openbitfun_codex_adapter::BuiltinPetCatalog as ExternalBuiltinPetCatalog;

pub fn external_builtin_pet_sources(
    ecosystem_id: &str,
    only_id: Option<&str>,
) -> ExternalBuiltinPetCatalog {
    match ecosystem_id {
        "codex" => openbitfun_codex_adapter::builtin_pet_sources(only_id),
        _ => ExternalBuiltinPetCatalog {
            pets: Vec::new(),
            diagnostics: vec!["Built-in pet source is unsupported".into()],
        },
    }
}

pub fn external_pet_source_root(ecosystem_id: &str) -> Option<PathBuf> {
    match ecosystem_id {
        "codex" => Some(openbitfun_codex_adapter::pet_source_root()),
        _ => None,
    }
}

const PROVIDER_DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const EXTERNAL_SOURCE_PREFERENCES_FILE: &str = "external-sources.json";
const SUBAGENT_CONFLICT_RESELECTION_REQUIRED: &str = "__openbitfun_reselection_required__";
const OPENCODE_ECOSYSTEM_ID: &str = "opencode";
const CLAUDE_CODE_ECOSYSTEM_ID: &str = "claude-code";
const CODEX_ECOSYSTEM_ID: &str = "codex";
pub const EXTERNAL_CAPABILITY_COMMAND: &str = "command";
pub const EXTERNAL_CAPABILITY_TOOL: &str = "tool";
pub const EXTERNAL_CAPABILITY_SUBAGENT: &str = "subagent";
pub const EXTERNAL_CAPABILITY_MCP: &str = "mcp";
pub const EXTERNAL_CAPABILITY_REFERENCE: &str = "reference";
const EXTERNAL_ADAPTER_CONTRACT_MAJOR: u32 = 1;
const MAX_PROMPT_COMMAND_FILE_REFERENCES: usize = 8;
const MAX_PROMPT_COMMAND_FILE_BYTES: usize = 64 * 1024;
const MAX_PROMPT_COMMAND_TOTAL_FILE_BYTES: usize = 128 * 1024;
const MAX_EXPANDED_PROMPT_COMMAND_BYTES: usize = 1024 * 1024;
const PROMPT_COMMAND_SHELL_REVIEW_SCHEMA_VERSION: u32 = 1;
const MAX_PROMPT_COMMAND_SHELL_INVOCATIONS: usize = 8;
const MAX_PROMPT_COMMAND_SHELL_COMMAND_BYTES: usize = 64 * 1024;
const MAX_PROMPT_COMMAND_SHELL_TOTAL_BYTES: usize = 128 * 1024;
const MAX_PROMPT_COMMAND_SHELL_OUTPUT_CHARS: usize = 256 * 1024;
const PROMPT_COMMAND_SHELL_TIMEOUT_MS: u64 = 30_000;
const PROMPT_COMMAND_SHELL_KILL_YIELD_MS: u64 = 5_000;
const MAX_APPROVED_PROMPT_COMMAND_SHELL_PLANS: usize = 512;
/// Awareness records are tiny and bounded by the number of registered
/// ecosystems, but the cap keeps a corrupted or hostile file from growing
/// without limit.
const MAX_ACKNOWLEDGED_ECOSYSTEMS: usize = 256;
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedPromptCommandShell {
    display_name: String,
    path: PathBuf,
    kind: ExecCommandShellKind,
}

#[derive(Debug, Clone)]
struct PreparedPromptCommandShellPlan {
    expansion: PromptCommandExpansion,
    invocations: Vec<PromptCommandShellInvocation>,
    working_directory: PathBuf,
    resolved_shell: ResolvedPromptCommandShell,
    review: PromptCommandShellReviewPlan,
}

fn prepare_prompt_command_shell_plan(
    expansion: PromptCommandExpansion,
    source_display_name: &str,
    execution_domain_id: &str,
    candidate_id: &str,
    content_version: &str,
    preference_revision: u64,
    resolved_shell: ResolvedPromptCommandShell,
) -> Result<PreparedPromptCommandShellPlan, String> {
    let shell = expansion
        .shell
        .as_ref()
        .ok_or_else(|| "prompt command does not contain shell directives".to_string())?;
    if !shell.working_directory.is_absolute() {
        return Err("prompt command shell working directory must be absolute".to_string());
    }
    if shell.invocations.is_empty()
        || shell.invocations.len() > MAX_PROMPT_COMMAND_SHELL_INVOCATIONS
    {
        return Err(format!(
            "prompt commands may contain at most {MAX_PROMPT_COMMAND_SHELL_INVOCATIONS} shell directives"
        ));
    }
    let mut previous_end = 0usize;
    let mut total_bytes = 0usize;
    for invocation in &shell.invocations {
        let directive = expansion
            .content
            .get(invocation.range_start..invocation.range_end)
            .ok_or_else(|| "prompt command shell directive range is invalid".to_string())?;
        if invocation.range_start < previous_end
            || !directive.starts_with("!`")
            || !directive.ends_with('`')
            || directive.get(2..directive.len().saturating_sub(1))
                != Some(invocation.command.as_str())
        {
            return Err("prompt command shell directive range is inconsistent".to_string());
        }
        if invocation.command.len() > MAX_PROMPT_COMMAND_SHELL_COMMAND_BYTES {
            return Err(format!(
                "a prompt command shell directive exceeds the {MAX_PROMPT_COMMAND_SHELL_COMMAND_BYTES} byte limit"
            ));
        }
        total_bytes = total_bytes
            .checked_add(invocation.command.len())
            .ok_or_else(|| {
                "prompt command shell directives exceed the total byte limit".to_string()
            })?;
        previous_end = invocation.range_end;
    }
    if total_bytes > MAX_PROMPT_COMMAND_SHELL_TOTAL_BYTES {
        return Err(format!(
            "prompt command shell directives exceed the {MAX_PROMPT_COMMAND_SHELL_TOTAL_BYTES} byte total limit"
        ));
    }

    let plan_fingerprint = prompt_command_shell_plan_fingerprint(
        execution_domain_id,
        candidate_id,
        content_version,
        &shell.working_directory,
        &resolved_shell,
        &shell.invocations,
    );
    let review = PromptCommandShellReviewPlan {
        schema_version: PROMPT_COMMAND_SHELL_REVIEW_SCHEMA_VERSION,
        plan_fingerprint,
        source_display_name: source_display_name.to_string(),
        working_directory: shell.working_directory.to_string_lossy().to_string(),
        shell_display_name: resolved_shell.display_name.clone(),
        shell_executable: resolved_shell
            .path
            .to_str()
            .ok_or_else(|| "prompt command shell executable path is not valid Unicode".to_string())?
            .to_string(),
        commands: shell
            .invocations
            .iter()
            .map(|invocation| invocation.command.clone())
            .collect(),
        can_remember: shell
            .invocations
            .iter()
            .all(|invocation| invocation.can_remember),
        preference_revision,
    };
    Ok(PreparedPromptCommandShellPlan {
        invocations: shell.invocations.clone(),
        working_directory: shell.working_directory.clone(),
        expansion,
        resolved_shell,
        review,
    })
}

fn prompt_command_shell_plan_fingerprint(
    execution_domain_id: &str,
    candidate_id: &str,
    content_version: &str,
    working_directory: &Path,
    shell: &ResolvedPromptCommandShell,
    invocations: &[PromptCommandShellInvocation],
) -> String {
    let mut hasher = Sha256::new();
    for part in [
        PROMPT_COMMAND_SHELL_REVIEW_SCHEMA_VERSION.to_string(),
        execution_domain_id.to_string(),
        candidate_id.to_string(),
        content_version.to_string(),
        prompt_command_shell_kind_id(&shell.kind),
    ] {
        update_prompt_command_shell_fingerprint(&mut hasher, part.as_bytes());
    }
    update_prompt_command_shell_fingerprint(
        &mut hasher,
        &prompt_command_shell_path_bytes(working_directory),
    );
    update_prompt_command_shell_fingerprint(
        &mut hasher,
        &prompt_command_shell_path_bytes(&shell.path),
    );
    for invocation in invocations {
        update_prompt_command_shell_fingerprint(&mut hasher, invocation.command.as_bytes());
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn update_prompt_command_shell_fingerprint(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn prompt_command_shell_path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        return path.as_os_str().as_bytes().to_vec();
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        return path
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect();
    }
    #[cfg(not(any(unix, windows)))]
    path.to_string_lossy().as_bytes().to_vec()
}

fn prompt_command_shell_kind_id(kind: &ExecCommandShellKind) -> String {
    match kind {
        ExecCommandShellKind::Bash => "bash".to_string(),
        ExecCommandShellKind::Zsh => "zsh".to_string(),
        ExecCommandShellKind::Fish => "fish".to_string(),
        ExecCommandShellKind::PowerShell => "powershell".to_string(),
        ExecCommandShellKind::PowerShellCore => "powershell_core".to_string(),
        ExecCommandShellKind::Cmd => "cmd".to_string(),
        ExecCommandShellKind::Sh => "sh".to_string(),
        ExecCommandShellKind::Ksh => "ksh".to_string(),
        ExecCommandShellKind::Csh => "csh".to_string(),
        ExecCommandShellKind::Custom(name) => format!("custom:{name}"),
    }
}

fn apply_prompt_command_shell_outputs(
    content: &str,
    invocations: &[PromptCommandShellInvocation],
    outputs: &[String],
) -> Result<String, String> {
    if invocations.len() != outputs.len() {
        return Err("prompt command shell output count is inconsistent".to_string());
    }
    let output_bytes = outputs.iter().map(String::len).sum::<usize>();
    let removed_bytes = invocations
        .iter()
        .map(|invocation| invocation.range_end.saturating_sub(invocation.range_start))
        .sum::<usize>();
    let capacity = content
        .len()
        .saturating_sub(removed_bytes)
        .saturating_add(output_bytes);
    if capacity > MAX_EXPANDED_PROMPT_COMMAND_BYTES {
        return Err(format!(
            "expanded external prompt command exceeds the {MAX_EXPANDED_PROMPT_COMMAND_BYTES} byte limit"
        ));
    }
    let mut expanded = String::with_capacity(capacity);
    let mut cursor = 0usize;
    for (invocation, output) in invocations.iter().zip(outputs) {
        let prefix = content
            .get(cursor..invocation.range_start)
            .ok_or_else(|| "prompt command shell directive range is invalid".to_string())?;
        expanded.push_str(prefix);
        expanded.push_str(output);
        cursor = invocation.range_end;
    }
    expanded.push_str(
        content
            .get(cursor..)
            .ok_or_else(|| "prompt command shell directive range is invalid".to_string())?,
    );
    Ok(expanded)
}

fn resolve_prompt_command_shell(
    preference: &PromptCommandShellPreference,
) -> Result<ResolvedPromptCommandShell, String> {
    let resolved = match preference {
        PromptCommandShellPreference::HostDefault => {
            let shell = resolve_local_exec_shell_without_probe(None);
            return finalize_resolved_prompt_command_shell(
                shell.display_name,
                shell.path,
                shell.shell_type,
            );
        }
        PromptCommandShellPreference::Preferred { executable } => {
            if let Some(shell) = ShellDetector::resolve_configured_shell_without_probe(executable) {
                return finalize_resolved_prompt_command_shell(
                    shell.display_name,
                    shell.path,
                    shell.shell_type,
                );
            }
            let fallback = resolve_local_exec_shell_without_probe(None);
            return finalize_resolved_prompt_command_shell(
                fallback.display_name,
                fallback.path,
                fallback.shell_type,
            );
        }
        PromptCommandShellPreference::Required { executable } => {
            ShellDetector::resolve_configured_shell_without_probe(executable).ok_or_else(|| {
                format!("required prompt command shell '{executable}' is unavailable")
            })?
        }
        PromptCommandShellPreference::RequiredOneOf { executables } => {
            let mut resolved = None;
            for executable in executables {
                if let Some(shell) =
                    ShellDetector::resolve_configured_shell_without_probe(executable)
                {
                    resolved = Some(shell);
                    break;
                }
            }
            resolved.ok_or_else(|| {
                "none of the required prompt command shell candidates are available".to_string()
            })?
        }
    };
    finalize_resolved_prompt_command_shell(
        resolved.display_name,
        resolved.path,
        resolved.shell_type,
    )
}

fn finalize_resolved_prompt_command_shell(
    display_name: String,
    path: PathBuf,
    shell_type: ShellType,
) -> Result<ResolvedPromptCommandShell, String> {
    let path = dunce::canonicalize(&path)
        .map_err(|_| "prompt command shell executable is unavailable".to_string())?;
    if path.to_str().is_none() {
        return Err("prompt command shell executable path is not valid Unicode".to_string());
    }
    Ok(ResolvedPromptCommandShell {
        display_name,
        path,
        kind: prompt_command_shell_kind(&shell_type),
    })
}

fn prompt_command_shell_kind(shell_type: &ShellType) -> ExecCommandShellKind {
    match shell_type {
        ShellType::Bash => ExecCommandShellKind::Bash,
        ShellType::Zsh => ExecCommandShellKind::Zsh,
        ShellType::Fish => ExecCommandShellKind::Fish,
        ShellType::PowerShell => ExecCommandShellKind::PowerShell,
        ShellType::PowerShellCore => ExecCommandShellKind::PowerShellCore,
        ShellType::Cmd => ExecCommandShellKind::Cmd,
        ShellType::Sh => ExecCommandShellKind::Sh,
        ShellType::Ksh => ExecCommandShellKind::Ksh,
        ShellType::Csh => ExecCommandShellKind::Csh,
        ShellType::Custom(name) => ExecCommandShellKind::Custom(name.clone()),
    }
}

async fn execute_prompt_command_shell_plan(
    plan: PreparedPromptCommandShellPlan,
) -> Result<PromptCommandExpansion, String> {
    if !plan.working_directory.is_dir() {
        return Err("prompt command shell working directory is unavailable".to_string());
    }
    let manager = Arc::new(ExecProcessManager::default());
    let tasks = plan
        .invocations
        .iter()
        .enumerate()
        .map(|(index, invocation)| {
            let manager = Arc::clone(&manager);
            let argv = exec_command_argv_for_isolated_shell(
                plan.resolved_shell
                    .path
                    .to_str()
                    .expect("prepared prompt command shell paths are valid Unicode")
                    .to_string(),
                plan.resolved_shell.kind.clone(),
                &invocation.command,
            );
            let cwd = plan.working_directory.clone();
            async move {
                let response = manager
                    .exec_command_stdout(LocalExecCommandRequest {
                        argv,
                        cwd,
                        env: exec_command_noninteractive_env(),
                        tty: false,
                        yield_time_ms: Some(PROMPT_COMMAND_SHELL_TIMEOUT_MS),
                        max_output_chars: Some(MAX_PROMPT_COMMAND_SHELL_OUTPUT_CHARS),
                        wait_interrupt: None,
                        lifecycle_tx: None,
                        output_capture_tx: None,
                    })
                    .await
                    .map_err(|error| {
                        format!(
                            "prompt command shell directive {} failed to start: {error}",
                            index + 1
                        )
                    })?;
                if let Some(session_id) = response.session_id {
                    let _ = manager
                        .control_session(ExecControlRequest {
                            session_id,
                            action: ExecControlAction::Kill,
                            origin: ExecControlOrigin::OutOfBand,
                            yield_time_ms: Some(PROMPT_COMMAND_SHELL_KILL_YIELD_MS),
                            max_output_chars: Some(MAX_PROMPT_COMMAND_SHELL_OUTPUT_CHARS),
                        })
                        .await;
                    if response.original_output_chars >= MAX_PROMPT_COMMAND_SHELL_OUTPUT_CHARS {
                        return Err(format!(
                            "prompt command shell directive {} exceeded the output limit",
                            index + 1
                        ));
                    }
                    return Err(format!(
                        "prompt command shell directive {} exceeded the {} second timeout",
                        index + 1,
                        PROMPT_COMMAND_SHELL_TIMEOUT_MS / 1000
                    ));
                }
                if response.original_output_chars > MAX_PROMPT_COMMAND_SHELL_OUTPUT_CHARS {
                    return Err(format!(
                        "prompt command shell directive {} exceeded the output limit",
                        index + 1
                    ));
                }
                Ok(response.output)
            }
        })
        .collect::<Vec<_>>();
    let outputs = join_all(tasks)
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    let content =
        apply_prompt_command_shell_outputs(&plan.expansion.content, &plan.invocations, &outputs)?;
    Ok(PromptCommandExpansion {
        content,
        workspace_file_references: plan.expansion.workspace_file_references,
        shell: None,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct LocalConfiguredSkillRootContribution {
    pub path: PathBuf,
    pub scope: ExternalSourceScope,
    pub precedence: usize,
}

pub(crate) fn opencode_configured_skill_roots(
    workspace_root: Option<&Path>,
) -> (
    Vec<LocalConfiguredSkillRootContribution>,
    Vec<(String, String)>,
) {
    opencode_configured_skill_roots_with_provider(
        workspace_root,
        &OpenCodeSkillRootProvider::default(),
    )
}

pub(crate) fn pi_configured_skill_roots(
    workspace_root: Option<&Path>,
) -> (
    Vec<LocalConfiguredSkillRootContribution>,
    Vec<(String, String)>,
) {
    let report = openbitfun_pi_adapter::PiSkillRootProvider::default().discover(workspace_root);
    let roots = report
        .roots
        .into_iter()
        .enumerate()
        .map(|(precedence, root)| LocalConfiguredSkillRootContribution {
            path: root.path,
            scope: root.scope,
            precedence,
        })
        .collect();
    (roots, report.diagnostics)
}

fn opencode_configured_skill_roots_with_provider(
    workspace_root: Option<&Path>,
    provider: &OpenCodeSkillRootProvider,
) -> (
    Vec<LocalConfiguredSkillRootContribution>,
    Vec<(String, String)>,
) {
    let report = provider.discover_with_diagnostics(workspace_root);
    let roots = report
        .roots
        .into_iter()
        .map(|root| LocalConfiguredSkillRootContribution {
            path: root.path,
            scope: root.scope,
            precedence: root.precedence,
        })
        .collect();
    (
        roots,
        report
            .diagnostics
            .into_iter()
            .map(|diagnostic| (diagnostic.location, diagnostic.message))
            .collect(),
    )
}

async fn finalize_prompt_command_expansion(
    workspace_root: Option<&Path>,
    expansion: PromptCommandExpansion,
) -> Result<ExpandedPromptCommand, String> {
    if expansion.content.len() > MAX_EXPANDED_PROMPT_COMMAND_BYTES {
        return Err(format!(
            "expanded external prompt command exceeds the {MAX_EXPANDED_PROMPT_COMMAND_BYTES} byte limit"
        ));
    }
    if expansion.workspace_file_references.is_empty() {
        return Ok(ExpandedPromptCommand {
            content: expansion.content,
        });
    }

    let workspace_root = workspace_root.ok_or_else(|| {
        "external prompt command file reference expansion requires a local workspace".to_string()
    })?;
    let mut seen = BTreeSet::new();
    let references = expansion
        .workspace_file_references
        .into_iter()
        .filter(|reference| seen.insert(reference.clone()))
        .collect::<Vec<_>>();
    if references.len() > MAX_PROMPT_COMMAND_FILE_REFERENCES {
        return Err(format!(
            "external prompt commands may reference at most {MAX_PROMPT_COMMAND_FILE_REFERENCES} workspace files"
        ));
    }

    let mut total_file_bytes = 0usize;
    let mut files = Vec::with_capacity(references.len());
    for reference in references {
        let file = read_workspace_relative_text_bounded(
            workspace_root,
            &reference,
            MAX_PROMPT_COMMAND_FILE_BYTES,
        )
        .await
        .map_err(|error| {
            format!("failed to read referenced workspace file '{reference}': {error}")
        })?;
        total_file_bytes = total_file_bytes
            .checked_add(file.byte_len)
            .ok_or_else(|| "referenced workspace files exceed the total byte limit".to_string())?;
        if total_file_bytes > MAX_PROMPT_COMMAND_TOTAL_FILE_BYTES {
            return Err(format!(
                "referenced workspace files exceed the {MAX_PROMPT_COMMAND_TOTAL_FILE_BYTES} byte total limit"
            ));
        }
        files.push(file);
    }

    let mut content = expansion.content;
    content.push_str("\n\n## Referenced workspace files");
    for file in files {
        let fence = markdown_code_fence(&file.content);
        content.push_str("\n\n### `");
        content.push_str(&file.relative_path);
        content.push_str("`\n\n");
        content.push_str(&fence);
        content.push_str("text\n");
        content.push_str(&file.content);
        if !file.content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&fence);
    }
    if content.len() > MAX_EXPANDED_PROMPT_COMMAND_BYTES {
        return Err(format!(
            "expanded external prompt command exceeds the {MAX_EXPANDED_PROMPT_COMMAND_BYTES} byte limit"
        ));
    }
    Ok(ExpandedPromptCommand { content })
}

fn markdown_code_fence(content: &str) -> String {
    let longest_run = content
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    "`".repeat(longest_run.saturating_add(1).max(3))
}

fn external_capability_descriptor(
    capability_id: &str,
    recommended_access: ExternalIntegrationAccess,
    safety_ceiling: ExternalIntegrationAccess,
) -> ExternalIntegrationCapabilityDescriptor {
    ExternalIntegrationCapabilityDescriptor {
        capability_id: ExternalIntegrationCapabilityId::new(capability_id)
            .expect("built-in external integration capability id is valid"),
        recommended_access,
        safety_ceiling,
    }
}

/// Internal SDK-ready registration seam. Adapters only contribute discovery
/// providers and metadata; execution remains owned by OpenBitFun policy/runtime.
#[derive(Clone)]
struct ExternalEcosystemRegistration {
    descriptor: ExternalIntegrationEcosystemDescriptor,
    contract_major: u32,
    upstream_format_revision: &'static str,
    command_provider: Option<Arc<dyn PromptCommandSourceProvider>>,
    tool_provider: Option<Arc<dyn ExternalToolSourceProvider>>,
    subagent_provider: Option<Arc<dyn ExternalSubagentSourceProvider>>,
    mcp_provider: Option<Arc<dyn ExternalMcpSourceProvider>>,
    workspace_reference_provider: Option<Arc<dyn ExternalWorkspaceReferenceSourceProvider>>,
}

impl ExternalEcosystemRegistration {
    fn validate(&self) -> Result<(), String> {
        if self.contract_major != EXTERNAL_ADAPTER_CONTRACT_MAJOR {
            return Err(format!(
                "adapter contract major {} is not supported",
                self.contract_major
            ));
        }
        let ecosystem_id = &self.descriptor.ecosystem_id;
        let capabilities = self
            .descriptor
            .capabilities
            .iter()
            .map(|capability| capability.capability_id.as_str())
            .collect::<BTreeSet<_>>();
        let providers = [
            (
                EXTERNAL_CAPABILITY_COMMAND,
                self.command_provider
                    .as_ref()
                    .map(|provider| provider.identity().ecosystem_id),
            ),
            (
                EXTERNAL_CAPABILITY_TOOL,
                self.tool_provider
                    .as_ref()
                    .map(|provider| provider.identity().ecosystem_id),
            ),
            (
                EXTERNAL_CAPABILITY_SUBAGENT,
                self.subagent_provider
                    .as_ref()
                    .map(|provider| provider.identity().ecosystem_id),
            ),
            (
                EXTERNAL_CAPABILITY_MCP,
                self.mcp_provider
                    .as_ref()
                    .map(|provider| provider.identity().ecosystem_id),
            ),
            (
                EXTERNAL_CAPABILITY_REFERENCE,
                self.workspace_reference_provider
                    .as_ref()
                    .map(|provider| provider.identity().ecosystem_id),
            ),
        ];
        for (capability_id, provider_ecosystem) in providers {
            if capabilities.contains(capability_id) != provider_ecosystem.is_some() {
                return Err(format!(
                    "capability '{capability_id}' and provider registration do not match"
                ));
            }
            if provider_ecosystem
                .as_ref()
                .is_some_and(|provider_ecosystem| provider_ecosystem != ecosystem_id)
            {
                return Err(format!(
                    "capability '{capability_id}' provider belongs to a different ecosystem"
                ));
            }
        }
        Ok(())
    }
}

fn default_external_integration_registry() -> Vec<ExternalEcosystemRegistration> {
    vec![
        ExternalEcosystemRegistration {
            descriptor: ExternalIntegrationEcosystemDescriptor {
                ecosystem_id: EcosystemId::new(OPENCODE_ECOSYSTEM_ID)
                    .expect("OpenCode ecosystem id is valid"),
                display_name: "OpenCode".to_string(),
                adapter_revision: "1".to_string(),
                capabilities: vec![
                    external_capability_descriptor(
                        EXTERNAL_CAPABILITY_COMMAND,
                        ExternalIntegrationAccess::Auto,
                        ExternalIntegrationAccess::Auto,
                    ),
                    external_capability_descriptor(
                        EXTERNAL_CAPABILITY_TOOL,
                        ExternalIntegrationAccess::AskBeforeUse,
                        ExternalIntegrationAccess::AskBeforeUse,
                    ),
                    external_capability_descriptor(
                        EXTERNAL_CAPABILITY_SUBAGENT,
                        ExternalIntegrationAccess::AskBeforeUse,
                        ExternalIntegrationAccess::AskBeforeUse,
                    ),
                    external_capability_descriptor(
                        EXTERNAL_CAPABILITY_MCP,
                        ExternalIntegrationAccess::AskBeforeUse,
                        ExternalIntegrationAccess::AskBeforeUse,
                    ),
                    external_capability_descriptor(
                        EXTERNAL_CAPABILITY_REFERENCE,
                        ExternalIntegrationAccess::Auto,
                        ExternalIntegrationAccess::Auto,
                    ),
                ],
            },
            contract_major: EXTERNAL_ADAPTER_CONTRACT_MAJOR,
            upstream_format_revision: "opencode-config-v1",
            command_provider: Some(Arc::new(OpenCodeCommandProvider::default())),
            tool_provider: Some(Arc::new(OpenCodeToolProvider::default())),
            subagent_provider: Some(Arc::new(OpenCodeSubagentProvider::default())),
            mcp_provider: Some(Arc::new(OpenCodeMcpProvider::default())),
            workspace_reference_provider: Some(Arc::new(
                OpenCodeWorkspaceReferenceProvider::default(),
            )),
        },
        ExternalEcosystemRegistration {
            descriptor: ExternalIntegrationEcosystemDescriptor {
                ecosystem_id: EcosystemId::new(CLAUDE_CODE_ECOSYSTEM_ID)
                    .expect("Claude Code ecosystem id is valid"),
                display_name: "Claude Code".to_string(),
                adapter_revision: "1".to_string(),
                capabilities: vec![
                    external_capability_descriptor(
                        EXTERNAL_CAPABILITY_COMMAND,
                        ExternalIntegrationAccess::Auto,
                        ExternalIntegrationAccess::Auto,
                    ),
                    external_capability_descriptor(
                        EXTERNAL_CAPABILITY_SUBAGENT,
                        ExternalIntegrationAccess::AskBeforeUse,
                        ExternalIntegrationAccess::AskBeforeUse,
                    ),
                    external_capability_descriptor(
                        EXTERNAL_CAPABILITY_MCP,
                        ExternalIntegrationAccess::AskBeforeUse,
                        ExternalIntegrationAccess::AskBeforeUse,
                    ),
                ],
            },
            contract_major: EXTERNAL_ADAPTER_CONTRACT_MAJOR,
            upstream_format_revision: "claude-code-config-v1",
            command_provider: Some(Arc::new(ClaudeCodeCommandProvider::default())),
            tool_provider: None,
            subagent_provider: Some(Arc::new(ClaudeCodeSubagentProvider::default())),
            mcp_provider: Some(Arc::new(ClaudeCodeMcpProvider::default())),
            workspace_reference_provider: None,
        },
        ExternalEcosystemRegistration {
            descriptor: ExternalIntegrationEcosystemDescriptor {
                ecosystem_id: EcosystemId::new(CODEX_ECOSYSTEM_ID)
                    .expect("Codex ecosystem id is valid"),
                display_name: "Codex".to_string(),
                adapter_revision: "1".to_string(),
                capabilities: vec![
                    external_capability_descriptor(
                        EXTERNAL_CAPABILITY_SUBAGENT,
                        ExternalIntegrationAccess::AskBeforeUse,
                        ExternalIntegrationAccess::AskBeforeUse,
                    ),
                    external_capability_descriptor(
                        EXTERNAL_CAPABILITY_MCP,
                        ExternalIntegrationAccess::AskBeforeUse,
                        ExternalIntegrationAccess::AskBeforeUse,
                    ),
                ],
            },
            contract_major: EXTERNAL_ADAPTER_CONTRACT_MAJOR,
            upstream_format_revision: "codex-config-v1",
            command_provider: None,
            tool_provider: None,
            subagent_provider: Some(Arc::new(CodexSubagentProvider::default())),
            mcp_provider: Some(Arc::new(CodexMcpProvider::default())),
            workspace_reference_provider: None,
        },
        ExternalEcosystemRegistration {
            descriptor: ExternalIntegrationEcosystemDescriptor {
                ecosystem_id: EcosystemId::new("deepseek-harness").expect("static ecosystem id"),
                display_name: "DeepSeek Harness".to_string(),
                adapter_revision: "1".to_string(),
                capabilities: vec![external_capability_descriptor(
                    EXTERNAL_CAPABILITY_MCP,
                    ExternalIntegrationAccess::AskBeforeUse,
                    ExternalIntegrationAccess::AskBeforeUse,
                )],
            },
            contract_major: EXTERNAL_ADAPTER_CONTRACT_MAJOR,
            upstream_format_revision: "dsh-mcp-declarations-v1",
            command_provider: None,
            tool_provider: None,
            subagent_provider: None,
            mcp_provider: Some(Arc::new(DshMcpProvider::default())),
            workspace_reference_provider: None,
        },
    ]
}

/// Resolve legacy native import receipts using registered provider identity, even
/// when the external source is offline or no longer present. This does no discovery.
pub fn ecosystem_for_imported_mcp_candidate(candidate_id: &str) -> Option<String> {
    let qualified =
        openbitfun_product_domains::external_sources::SourceQualifiedMcpServerId::from_stable_key(
            candidate_id.strip_prefix("external_mcp:")?,
        )?;
    default_external_integration_registry()
        .into_iter()
        .find_map(|registration| {
            let provider = registration.mcp_provider?;
            let identity = provider.identity();
            (identity.provider_id == qualified.source.provider_id)
                .then(|| identity.ecosystem_id.to_string())
        })
}

fn default_external_integration_ecosystems() -> Vec<ExternalIntegrationEcosystemDescriptor> {
    default_external_integration_registry()
        .into_iter()
        .filter(|registration| {
            let compatible = registration.validate();
            if let Err(error) = &compatible {
                log::warn!(
                    "External ecosystem adapter skipped ecosystem={} contract_major={} host_contract_major={} upstream_format={} reason={}",
                    safe_external_log_token(registration.descriptor.ecosystem_id.as_str()),
                    registration.contract_major,
                    EXTERNAL_ADAPTER_CONTRACT_MAJOR,
                    safe_external_log_token(registration.upstream_format_revision),
                    safe_external_log_token(error),
                );
            }
            compatible.is_ok()
        })
        .map(|registration| registration.descriptor)
        .collect()
}
/// Kept stable so existing approval fingerprints remain valid. Product hosts
/// resolve this identity once; capability owners never hard-code it.
const LEGACY_LOCAL_EXECUTION_DOMAIN_ID: &str = "local-user";

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ExternalSourcesConfig {
    #[serde(default)]
    integration_policy: StoredExternalIntegrationPolicy,
    /// Bounded recovery history for a policy document written by an
    /// incompatible host. This remains persistence-only and is never projected
    /// through public Host APIs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    integration_policy_backups: Vec<serde_json::Value>,
    /// Host-private entropy for opaque MCP configuration revisions. It is
    /// persisted locally but never projected through a public snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mcp_revision_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    suppressed_source_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    conflict_choices: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    conflict_lineage_current_keys: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    conflicted_candidate_ids: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    approved_tool_targets: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    approved_prompt_command_shell_plans: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    declined_tool_decisions: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    tool_conflict_choices: BTreeMap<String, String>,
    #[serde(default)]
    preference_revision: u64,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    approved_subagent_envelopes: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    declined_subagent_decisions: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    subagent_conflict_choices: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    subagent_conflict_lineage_current_keys: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    subagent_model_bindings: BTreeMap<String, ExternalSubagentModelBindingTarget>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    mcp_server_decisions: BTreeMap<String, ExternalMcpDecision>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    mcp_conflict_choices: BTreeMap<String, String>,
    /// Ecosystems the user has already been told about, as
    /// `execution_domain_id` + unit separator + `ecosystem_id`.
    ///
    /// This records awareness, not a policy decision: it only suppresses the
    /// "a new external application was found" hint. It deliberately carries no
    /// content version, because discovering more commands inside an ecosystem
    /// the user already knows about is not new information. Awareness is also
    /// user-wide rather than per workspace, so opening another project does not
    /// re-announce the same application.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    acknowledged_ecosystems: BTreeSet<String>,
    /// Preserves fields written by a newer preferences schema.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    extensions: BTreeMap<String, serde_json::Value>,
}

impl std::fmt::Debug for ExternalSourcesConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalSourcesConfig")
            .field("integration_policy", &self.integration_policy)
            .field(
                "integration_policy_backups",
                &self.integration_policy_backups,
            )
            .field(
                "mcp_revision_secret",
                &self.mcp_revision_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("suppressed_source_keys", &self.suppressed_source_keys)
            .field("conflict_choices", &self.conflict_choices)
            .field(
                "conflict_lineage_current_keys",
                &self.conflict_lineage_current_keys,
            )
            .field("conflicted_candidate_ids", &self.conflicted_candidate_ids)
            .field("approved_tool_targets", &self.approved_tool_targets)
            .field(
                "approved_prompt_command_shell_plans",
                &self.approved_prompt_command_shell_plans.len(),
            )
            .field("declined_tool_decisions", &self.declined_tool_decisions)
            .field("tool_conflict_choices", &self.tool_conflict_choices)
            .field("preference_revision", &self.preference_revision)
            .field(
                "approved_subagent_envelopes",
                &self.approved_subagent_envelopes,
            )
            .field(
                "declined_subagent_decisions",
                &self.declined_subagent_decisions,
            )
            .field("subagent_conflict_choices", &self.subagent_conflict_choices)
            .field(
                "subagent_conflict_lineage_current_keys",
                &self.subagent_conflict_lineage_current_keys,
            )
            .field("subagent_model_bindings", &self.subagent_model_bindings)
            .field("mcp_server_decisions", &self.mcp_server_decisions)
            .field("mcp_conflict_choices", &self.mcp_conflict_choices)
            .field("acknowledged_ecosystems", &self.acknowledged_ecosystems)
            .field("extensions", &self.extensions)
            .finish()
    }
}

/// Persistence-only version gate. Unknown major versions remain opaque until
/// the user explicitly backs them up and resets; this prevents current structs
/// from partially decoding a future policy shape before compatibility is known.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StoredExternalIntegrationPolicy {
    Known(ExternalIntegrationPolicyDocument),
    Unknown {
        schema_major: u32,
        raw: serde_json::Value,
    },
}

impl StoredExternalIntegrationPolicy {
    fn schema_major(&self) -> u32 {
        match self {
            Self::Known(document) => document.schema_major,
            Self::Unknown { schema_major, .. } => *schema_major,
        }
    }

    fn known(&self) -> Option<&ExternalIntegrationPolicyDocument> {
        match self {
            Self::Known(document) => Some(document),
            Self::Unknown { .. } => None,
        }
    }

    fn known_mut(&mut self) -> Option<&mut ExternalIntegrationPolicyDocument> {
        match self {
            Self::Known(document) => Some(document),
            Self::Unknown { .. } => None,
        }
    }

    fn raw_value(&self) -> serde_json::Value {
        match self {
            Self::Known(document) => serde_json::to_value(document).unwrap_or_else(|_| {
                serde_json::json!({
                    "schemaMajor": EXTERNAL_INTEGRATION_POLICY_SCHEMA_MAJOR,
                    "userDefaults": { "enabled": false }
                })
            }),
            Self::Unknown { raw, .. } => raw.clone(),
        }
    }
}

impl Default for StoredExternalIntegrationPolicy {
    fn default() -> Self {
        Self::Known(ExternalIntegrationPolicyDocument::default())
    }
}

impl Serialize for StoredExternalIntegrationPolicy {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.raw_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StoredExternalIntegrationPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let raw = serde_json::Value::deserialize(deserializer)?;
        let schema_major = raw
            .get("schemaMajor")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(EXTERNAL_INTEGRATION_POLICY_SCHEMA_MAJOR);
        if schema_major != EXTERNAL_INTEGRATION_POLICY_SCHEMA_MAJOR {
            return Ok(Self::Unknown { schema_major, raw });
        }
        serde_json::from_value(raw)
            .map(Self::Known)
            .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone)]
struct ExternalSourcePreferenceStore {
    path: PathBuf,
}

fn retired_automatic_application_policy() -> ExternalIntegrationPolicyDocument {
    let mut policy = ExternalIntegrationPolicyDocument::default();
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
            .entry(EcosystemId::new(ecosystem).expect("built-in ecosystem id is valid"))
            .or_default()
            .mode = mode;
    }
    policy
}

fn normalize_retired_application_defaults(config: &mut ExternalSourcesConfig) {
    // The retired application setup enabled integrations automatically. Undo
    // only that exact untouched default; every user-authored deviation wins.
    let from_retired_automatic_setup = config
        .extensions
        .get("configOrigin")
        .and_then(serde_json::Value::as_str)
        == Some("fresh_v2");
    if !from_retired_automatic_setup {
        return;
    }

    // Consume the retired origin on the first persisted update, including
    // documents that already contain a user deviation or application choice.
    // Otherwise a later user-authored policy matching the old default could be
    // mistaken for untouched setup state and reset again.
    config.extensions.remove("configOrigin");
    let has_application_choice = match config.extensions.get("applicationConnections") {
        None => false,
        Some(serde_json::Value::Object(decisions)) => !decisions.is_empty(),
        Some(_) => true,
    };
    if has_application_choice {
        return;
    }
    if config.integration_policy.known() == Some(&retired_automatic_application_policy()) {
        config.integration_policy = StoredExternalIntegrationPolicy::default();
    }
}

impl ExternalSourcePreferenceStore {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn global() -> Result<Self, String> {
        let path_manager =
            crate::infrastructure::try_get_path_manager_arc().map_err(|error| error.to_string())?;
        Ok(Self::new(
            path_manager
                .user_config_dir()
                .join(EXTERNAL_SOURCE_PREFERENCES_FILE),
        ))
    }

    async fn upgrade_workspace_policy(&self, workspace_id: &str) -> Result<(), String> {
        let registry = crate::service::workspace::get_global_workspace_service()
            .ok_or("Workspace service is unavailable")?;
        let records = registry.list_workspace_infos().await;
        let key = workspace_policy_key(Some(workspace_id)).ok_or("Missing workspace policy key")?;
        let upgrade = |config: &mut ExternalSourcesConfig| -> Result<bool, String> {
            let Some(document) = config.integration_policy.known_mut() else {
                return Ok(false);
            };
            crate::service::workspace::legacy_compat::upgrade_external_policy_key(
                &mut document.workspace_overrides,
                &records,
                workspace_id,
                &key,
            )
        };
        let mut config = self.read().await?;
        if !upgrade(&mut config)? {
            return Ok(());
        }
        let (result, _) = self
            .update(|config| {
                let changed = upgrade(config)?;
                if changed {
                    config.preference_revision += 1;
                }
                Ok::<(), String>(())
            })
            .await?;
        result
    }

    async fn read(&self) -> Result<ExternalSourcesConfig, String> {
        JsonFileStore
            .read_locked_optional(&self.path)
            .await
            .map(|config| {
                let mut config = config.unwrap_or_default();
                normalize_retired_application_defaults(&mut config);
                config
            })
            .map_err(|error| error.to_string())
    }

    async fn update<R>(
        &self,
        update: impl FnOnce(&mut ExternalSourcesConfig) -> R,
    ) -> Result<(R, ExternalSourcesConfig), String> {
        JsonFileStore
            .update_locked(&self.path, ExternalSourcesConfig::default(), |config| {
                normalize_retired_application_defaults(config);
                update(config)
            })
            .await
            .map_err(|error| error.to_string())
    }
}

fn decode_mcp_revision_key(value: &str) -> Option<ExternalMcpRevisionKey> {
    let bytes = hex::decode(value).ok()?;
    let bytes: [u8; 32] = bytes.try_into().ok()?;
    Some(ExternalMcpRevisionKey::new(bytes))
}

async fn external_sources_config_with_mcp_revision_key(
) -> Result<(ExternalSourcesConfig, ExternalMcpRevisionKey), String> {
    let store = ExternalSourcePreferenceStore::global()?;
    let config = store.read().await?;
    if let Some(revision_key) = config
        .mcp_revision_secret
        .as_deref()
        .and_then(decode_mcp_revision_key)
    {
        return Ok((config, revision_key));
    }
    let generated = {
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();
        let mut bytes = [0_u8; 32];
        bytes[..16].copy_from_slice(first.as_bytes());
        bytes[16..].copy_from_slice(second.as_bytes());
        bytes
    };
    let (_, config) = store
        .update(|config| {
            if config
                .mcp_revision_secret
                .as_deref()
                .and_then(decode_mcp_revision_key)
                .is_none()
            {
                config.mcp_revision_secret = Some(hex::encode(generated));
            }
        })
        .await?;
    let revision_key = config
        .mcp_revision_secret
        .as_deref()
        .and_then(decode_mcp_revision_key)
        .ok_or_else(|| "External MCP revision key could not be initialized".to_string())?;
    Ok((config, revision_key))
}

#[derive(Clone, Copy)]
enum WorkerRecoveryPolicy {
    Preserve,
    PendingOnce,
    ResetAndAttempt,
}

fn config_update_refreshes_external_model_bindings(event: &ConfigUpdateEvent) -> bool {
    matches!(event, ConfigUpdateEvent::ModelConfigurationUpdated)
}

pub(crate) fn host_execution_domain_id() -> Result<ExecutionDomainId, String> {
    ExecutionDomainId::new(LEGACY_LOCAL_EXECUTION_DOMAIN_ID).map_err(|error| error.to_string())
}

fn workspace_policy_key(workspace_id: Option<&str>) -> Option<String> {
    let route = workspace_route_key(workspace_id);
    workspace_policy_key_from_route(&route)
}

fn workspace_policy_key_from_route(route: &str) -> Option<String> {
    if route == "<global>" {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(route.as_bytes());
    Some(format!(
        "workspace:{}",
        hex::encode(&hasher.finalize()[..16])
    ))
}

fn external_source_safe_mode_key(execution_domain_id: &str, workspace_route: &str) -> String {
    format!("{execution_domain_id}\u{1f}{workspace_route}")
}

fn external_source_safe_mode_enabled_for(execution_domain_id: &str, workspace_route: &str) -> bool {
    safe_mode_workspaces().contains_key(&external_source_safe_mode_key(
        execution_domain_id,
        workspace_route,
    ))
}

fn set_external_source_safe_mode_for(
    execution_domain_id: &str,
    workspace_route: &str,
    enabled: bool,
) {
    let key = external_source_safe_mode_key(execution_domain_id, workspace_route);
    if enabled {
        safe_mode_workspaces().insert(key, ());
    } else {
        safe_mode_workspaces().remove(&key);
    }
}

fn integration_policy_snapshot(
    preferences: &ExternalSourcesConfig,
    workspace_id: Option<&str>,
) -> Result<ExternalIntegrationPolicySnapshot, String> {
    let ecosystems = default_external_integration_ecosystems();
    match preferences.integration_policy.known() {
        Some(document) => external_integration_policy_snapshot(
            document,
            workspace_policy_key(workspace_id).as_deref(),
            ecosystems,
        ),
        None => incompatible_external_integration_policy_snapshot(
            preferences.integration_policy.schema_major(),
            ecosystems,
        ),
    }
    .map_err(|error| format!("policy_unavailable: {error}"))
}

fn integration_access(
    policy: &ExternalIntegrationPolicySnapshot,
    ecosystem_id: &str,
    capability_id: &str,
) -> ExternalIntegrationAccess {
    let Some(ecosystem) = policy
        .effective
        .ecosystems
        .iter()
        .find_map(|(id, policy)| (id.as_str() == ecosystem_id).then_some(policy))
    else {
        return ExternalIntegrationAccess::Disabled;
    };
    ecosystem
        .capabilities
        .iter()
        .find_map(|(id, access)| (id.as_str() == capability_id).then_some(access.clone()))
        .unwrap_or(ExternalIntegrationAccess::Disabled)
}

fn integration_capability_is_discoverable(
    policy: &ExternalIntegrationPolicySnapshot,
    ecosystem_id: &str,
    capability_id: &str,
) -> bool {
    !matches!(
        integration_access(policy, ecosystem_id, capability_id),
        ExternalIntegrationAccess::Disabled | ExternalIntegrationAccess::Unknown(_)
    )
}

fn integration_capability_is_active(
    policy: &ExternalIntegrationPolicySnapshot,
    ecosystem_id: &str,
    capability_id: &str,
) -> bool {
    matches!(
        integration_access(policy, ecosystem_id, capability_id),
        ExternalIntegrationAccess::Auto | ExternalIntegrationAccess::AskBeforeUse
    )
}

fn ecosystems_with_discoverable_capability(
    policy: &ExternalIntegrationPolicySnapshot,
    capability_id: &str,
) -> BTreeSet<EcosystemId> {
    policy
        .registered_ecosystems
        .iter()
        .filter(|descriptor| {
            integration_capability_is_discoverable(
                policy,
                descriptor.ecosystem_id.as_str(),
                capability_id,
            )
        })
        .map(|descriptor| descriptor.ecosystem_id.clone())
        .collect()
}

fn ecosystems_with_active_capability(
    policy: &ExternalIntegrationPolicySnapshot,
    capability_id: &str,
) -> BTreeSet<EcosystemId> {
    policy
        .registered_ecosystems
        .iter()
        .filter(|descriptor| {
            integration_capability_is_active(
                policy,
                descriptor.ecosystem_id.as_str(),
                capability_id,
            )
        })
        .map(|descriptor| descriptor.ecosystem_id.clone())
        .collect()
}

fn source_ecosystem_id(
    snapshot: &ExternalSourceCatalogSnapshot,
    source_key: &SourceKey,
) -> Result<EcosystemId, String> {
    snapshot
        .sources
        .iter()
        .find(|source| source.record.key == *source_key)
        .map(|source| source.record.ecosystem_id.clone())
        .ok_or_else(|| {
            encoded_operation_error(
                ExternalSourceOperationErrorCode::NotFound,
                format!(
                    "External source '{}' is no longer available",
                    source_key.stable_key()
                ),
                false,
            )
        })
}

fn restrict_prompt_commands_without_active_subagents(
    commands: &mut [PromptCommandCatalogEntry],
    conflicts: &mut [PromptCommandConflict],
    active_subagents: &BTreeSet<(EcosystemId, String)>,
) {
    for command in commands {
        if !matches!(
            command.definition.availability,
            PromptCommandAvailability::Available
        ) {
            continue;
        }
        let PromptCommandExecutionTarget::FreshExternalSubagent {
            ecosystem_id,
            logical_id,
        } = &command.definition.execution_target
        else {
            continue;
        };
        let key = (ecosystem_id.clone(), logical_id.to_ascii_lowercase());
        if !active_subagents.contains(&key) {
            command.definition.availability = PromptCommandAvailability::Restricted {
                reason: format!(
                    "External command subagent '{}' is not currently approved and available",
                    logical_id
                ),
                required_capabilities: vec!["command.external_subagent".to_string()],
            };
        }
    }
    for conflict in conflicts {
        for candidate in &mut conflict.candidates {
            if !matches!(candidate.availability, PromptCommandAvailability::Available) {
                continue;
            }
            let PromptCommandExecutionTarget::FreshExternalSubagent {
                ecosystem_id,
                logical_id,
            } = &candidate.execution_target
            else {
                continue;
            };
            let key = (ecosystem_id.clone(), logical_id.to_ascii_lowercase());
            if !active_subagents.contains(&key) {
                candidate.availability = PromptCommandAvailability::Restricted {
                    reason: format!(
                        "External command subagent '{}' is not currently approved and available",
                        logical_id
                    ),
                    required_capabilities: vec!["command.external_subagent".to_string()],
                };
                if conflict.selected_candidate_id.as_deref()
                    == Some(candidate.candidate_id.as_str())
                {
                    conflict.selected_candidate_id = None;
                }
            }
        }
    }
}

fn ensure_source_capability_active(
    snapshot: &ExternalSourceCatalogSnapshot,
    source_key: &SourceKey,
    capability_id: &str,
) -> Result<(), String> {
    let ecosystem_id = source_ecosystem_id(snapshot, source_key)?;
    integration_capability_is_active(
        &snapshot.integration_policy,
        ecosystem_id.as_str(),
        capability_id,
    )
    .then_some(())
    .ok_or_else(|| encoded_operation_error(
        ExternalSourceOperationErrorCode::PolicyLimited,
        format!(
            "External capability '{capability_id}' is not enabled for ecosystem '{}' in this workspace",
            ecosystem_id.as_str()
        ),
        false,
    ))
}

fn ensure_source_set_capability_active(
    snapshot: &ExternalSourceCatalogSnapshot,
    source_keys: &[SourceKey],
    capability_id: &str,
) -> Result<(), String> {
    if source_keys.is_empty() {
        return Err(encoded_operation_error(
            ExternalSourceOperationErrorCode::NotFound,
            "External source provenance is missing",
            false,
        ));
    }
    for source_key in source_keys {
        ensure_source_capability_active(snapshot, source_key, capability_id)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalSourceServiceProfile {
    LocalExecution,
    ReadOnlyProjection,
    DiscoveryCatalog,
}

struct WorkspaceExternalSourceService {
    profile: ExternalSourceServiceProfile,
    workspace_id: Option<String>,
    workspace_root: Option<PathBuf>,
    execution_domain_id: ExecutionDomainId,
    mcp_revision_key: ExternalMcpRevisionKey,
    control_plane: Arc<ExternalSourceControlPlane>,
    snapshot: StdMutex<ExternalSourceCatalogSnapshot>,
    updates: broadcast::Sender<ExternalSourceCatalogSnapshot>,
    watch_states: tokio::sync::Mutex<BTreeMap<(PathBuf, bool), bool>>,
    refresh_gate: tokio::sync::Mutex<()>,
    product_rebuild_gate: tokio::sync::Mutex<()>,
    mcp_runtime: Arc<dyn ExternalMcpRuntimePort>,
    active_mcp_runtime_ids: tokio::sync::Mutex<BTreeSet<String>>,
    initial_refresh_completed: AtomicBool,
    automatic_discovery_active: AtomicBool,
    background_refresh_scheduled: AtomicBool,
    initial_refresh_gate: tokio::sync::Mutex<()>,
    keepalive_started: AtomicBool,
    last_access_epoch_seconds: AtomicU64,
    subagent_expiry_schedule: AtomicU64,
    watcher: Arc<FileWatchService>,
    #[cfg(test)]
    tool_decision_gate_waiting: tokio::sync::Notify,
    #[cfg(test)]
    tool_decision_gate_acquired: tokio::sync::Notify,
}

impl WorkspaceExternalSourceService {
    fn discovery_policy(
        &self,
        preferences: &ExternalSourcesConfig,
    ) -> Result<ExternalIntegrationPolicySnapshot, String> {
        if self.profile != ExternalSourceServiceProfile::DiscoveryCatalog {
            return integration_policy_snapshot(preferences, self.workspace_id.as_deref());
        }
        let Some(stored) = preferences.integration_policy.known() else {
            return integration_policy_snapshot(preferences, self.workspace_id.as_deref());
        };
        // This policy is used only by static discovery lanes. Runtime decisions
        // and the public policy snapshot continue to use the original document.
        let mut discovery = stored.clone();
        discovery.user_defaults.enabled = true;
        if let Some(key) = workspace_policy_key(self.workspace_id.as_deref()) {
            discovery
                .workspace_overrides
                .entry(key)
                .or_default()
                .enabled = Some(true);
        }
        external_integration_policy_snapshot(
            &discovery,
            workspace_policy_key(self.workspace_id.as_deref()).as_deref(),
            default_external_integration_ecosystems(),
        )
        .map_err(|error| format!("policy_unavailable: {error}"))
    }

    async fn automatic_discovery_allowed(&self) -> Result<bool, String> {
        if self.profile != ExternalSourceServiceProfile::DiscoveryCatalog {
            return Ok(true);
        }
        let preferences = read_external_sources_config().await?;
        Ok(preferences
            .integration_policy
            .known()
            .is_some_and(|document| {
                automatic_discovery_enabled(
                    document,
                    workspace_policy_key(self.workspace_id.as_deref()).as_deref(),
                )
            }))
    }

    async fn watch_policy(&self) -> Result<ExternalIntegrationPolicySnapshot, String> {
        let preferences = read_external_sources_config().await?;
        let mut policy = self.discovery_policy(&preferences)?;
        if self.profile == ExternalSourceServiceProfile::DiscoveryCatalog
            && !preferences
                .integration_policy
                .known()
                .is_some_and(|document| {
                    automatic_discovery_enabled(
                        document,
                        workspace_policy_key(self.workspace_id.as_deref()).as_deref(),
                    )
                })
        {
            policy.effective.enabled = false;
            policy.effective.ecosystems.clear();
        }
        Ok(policy)
    }

    async fn create(
        workspace_id: Option<String>,
        workspace_root: Option<PathBuf>,
        profile: ExternalSourceServiceProfile,
    ) -> Result<Arc<Self>, String> {
        let execution_domain_id = host_execution_domain_id()?;
        let context = ExternalSourceContext {
            workspace_root: workspace_root.clone(),
            execution_domain_id: execution_domain_id.clone(),
        };
        let registrations = default_external_integration_registry()
            .into_iter()
            .filter_map(|registration| match registration.validate() {
                Ok(()) => Some(registration),
                Err(error) => {
                    log::warn!(
                        "External ecosystem registration rejected ecosystem={} reason={}",
                        safe_external_log_token(registration.descriptor.ecosystem_id.as_str()),
                        safe_external_log_token(&error),
                    );
                    None
                }
            })
            .collect::<Vec<_>>();
        let providers: Vec<Arc<dyn PromptCommandSourceProvider>> = registrations
            .iter()
            .filter_map(|registration| registration.command_provider.as_ref().map(Arc::clone))
            .collect();
        let tool_providers: Vec<Arc<dyn ExternalToolSourceProvider>> = registrations
            .iter()
            .filter_map(|registration| registration.tool_provider.as_ref().map(Arc::clone))
            .collect();
        let subagent_providers: Vec<Arc<dyn ExternalSubagentSourceProvider>> = registrations
            .iter()
            .filter_map(|registration| registration.subagent_provider.as_ref().map(Arc::clone))
            .collect();
        let mcp_providers: Vec<Arc<dyn ExternalMcpSourceProvider>> = registrations
            .iter()
            .filter_map(|registration| registration.mcp_provider.as_ref().map(Arc::clone))
            .collect();
        let workspace_reference_providers: Vec<Arc<dyn ExternalWorkspaceReferenceSourceProvider>> =
            registrations
                .iter()
                .filter_map(|registration| {
                    registration
                        .workspace_reference_provider
                        .as_ref()
                        .map(Arc::clone)
                })
                .collect();
        let (preferences, mcp_revision_key) =
            external_sources_config_with_mcp_revision_key().await?;
        let control_plane = Arc::new(ExternalSourceControlPlane::new(
            context,
            mcp_revision_key.clone(),
            providers,
            tool_providers,
            subagent_providers,
            mcp_providers,
            workspace_reference_providers,
        )?);
        let suppressed_sources = preferences
            .suppressed_source_keys
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        control_plane.replace_suppressed_sources(suppressed_sources);
        control_plane.commands_mut(|coordinator| {
            coordinator.replace_conflict_choices(preferences.conflict_choices.clone());
            coordinator.replace_conflict_lineage_current_keys(
                preferences.conflict_lineage_current_keys.clone(),
            );
            coordinator
                .replace_conflicted_candidate_ids(preferences.conflicted_candidate_ids.clone());
        });
        let mut initial_snapshot = merge_tool_state(
            control_plane.commands(|coordinator| coordinator.snapshot()),
            &control_plane.tools(|coordinator| coordinator.snapshot()),
            ExternalToolProductState::default(),
        );
        initial_snapshot.subagent_generation =
            control_plane.subagents(|coordinator| coordinator.snapshot().generation);
        initial_snapshot.preference_revision = preferences.preference_revision;
        initial_snapshot.integration_policy =
            integration_policy_snapshot(&preferences, workspace_id.as_deref())?;
        let automatic_discovery = preferences
            .integration_policy
            .known()
            .is_some_and(|document| {
                automatic_discovery_enabled(
                    document,
                    workspace_policy_key(workspace_id.as_deref()).as_deref(),
                )
            });
        let (updates, _) = broadcast::channel(32);
        let service = Arc::new(Self {
            profile,
            workspace_id,
            workspace_root,
            execution_domain_id,
            mcp_revision_key,
            control_plane,
            snapshot: StdMutex::new(initial_snapshot),
            updates,
            watch_states: tokio::sync::Mutex::new(BTreeMap::new()),
            refresh_gate: tokio::sync::Mutex::new(()),
            product_rebuild_gate: tokio::sync::Mutex::new(()),
            mcp_runtime: Arc::new(OpenBitFunExternalMcpRuntime),
            active_mcp_runtime_ids: tokio::sync::Mutex::new(BTreeSet::new()),
            initial_refresh_completed: AtomicBool::new(false),
            automatic_discovery_active: AtomicBool::new(automatic_discovery),
            background_refresh_scheduled: AtomicBool::new(false),
            initial_refresh_gate: tokio::sync::Mutex::new(()),
            keepalive_started: AtomicBool::new(false),
            last_access_epoch_seconds: AtomicU64::new(epoch_seconds()),
            subagent_expiry_schedule: AtomicU64::new(0),
            watcher: Arc::new(FileWatchService::new(FileWatcherConfig::default())),
            #[cfg(test)]
            tool_decision_gate_waiting: tokio::sync::Notify::new(),
            #[cfg(test)]
            tool_decision_gate_acquired: tokio::sync::Notify::new(),
        });
        service.start_watching().await;
        if profile == ExternalSourceServiceProfile::LocalExecution {
            service.start_model_config_watching();
        }
        Ok(service)
    }

    async fn refresh(self: &Arc<Self>) -> Result<ExternalSourceCatalogSnapshot, String> {
        self.refresh_with_worker_recovery(WorkerRecoveryPolicy::ResetAndAttempt)
            .await
    }

    /// Snapshot imports need fresh MCP declarations, not unrelated command/tool,
    /// Agent or reference scans and their product/runtime reconciliation.
    async fn refresh_mcp_import_sources(self: &Arc<Self>) -> Result<(), String> {
        sync_service_preferences(self).await?;
        let preferences = read_external_sources_config().await?;
        let policy = self.discovery_policy(&preferences)?;
        self.discover_mcp_import_sources(&policy).await;
        Ok(())
    }

    async fn discover_mcp_import_sources(
        self: &Arc<Self>,
        policy: &ExternalIntegrationPolicySnapshot,
    ) {
        let _refresh_guard = self.refresh_gate.lock().await;
        let mut requests = Vec::new();
        let mut disabled_results = Vec::new();
        for request in lock_mcp_coordinator(&self.control_plane).discovery_requests() {
            if integration_capability_is_discoverable(
                policy,
                request.ecosystem_id().as_str(),
                EXTERNAL_CAPABILITY_MCP,
            ) {
                requests.push(request);
            } else {
                disabled_results.push(request.disabled());
            }
        }
        let mut batch = self
            .control_plane
            .discover_mcp(requests, PROVIDER_DISCOVERY_TIMEOUT)
            .await;
        batch.immediate.append(&mut disabled_results);
        lock_mcp_coordinator(&self.control_plane).apply_discovery_results(batch.immediate);
        for deferred in batch.deferred {
            self.schedule_deferred_mcp_discovery(deferred);
        }
    }

    async fn refresh_workspace_references(self: &Arc<Self>, force: bool) -> Result<(), String> {
        sync_service_preferences(self).await?;
        let _refresh_guard = self.refresh_gate.lock().await;
        if !force
            && !lock_workspace_reference_coordinator(&self.control_plane)
                .snapshot()
                .discovery_pending
        {
            return Ok(());
        }
        let preferences = read_external_sources_config().await?;
        let policy = self.discovery_policy(&preferences)?;
        let mut requests = Vec::new();
        let mut disabled_results = Vec::new();
        for request in
            lock_workspace_reference_coordinator(&self.control_plane).discovery_requests()
        {
            if integration_capability_is_discoverable(
                &policy,
                request.ecosystem_id().as_str(),
                EXTERNAL_CAPABILITY_REFERENCE,
            ) {
                requests.push(request);
            } else {
                disabled_results.push(request.disabled());
            }
        }
        let mut batch = self
            .control_plane
            .discover_workspace_references(requests, PROVIDER_DISCOVERY_TIMEOUT)
            .await;
        batch.immediate.append(&mut disabled_results);
        lock_workspace_reference_coordinator(&self.control_plane)
            .apply_discovery_results(batch.immediate);
        for deferred in batch.deferred {
            self.schedule_deferred_workspace_reference_discovery(deferred);
        }
        self.ensure_watch_roots(&policy).await;
        Ok(())
    }

    async fn ensure_workspace_reference_refresh(self: &Arc<Self>) -> Result<(), String> {
        let pending = lock_workspace_reference_coordinator(&self.control_plane)
            .snapshot()
            .discovery_pending;
        if pending {
            self.refresh_workspace_references(false).await?;
        }
        Ok(())
    }

    async fn refresh_with_runtime_invalidation(
        self: &Arc<Self>,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        if self.profile == ExternalSourceServiceProfile::LocalExecution {
            invalidate_external_tool_runtime_availability().await;
        }
        self.refresh().await
    }

    async fn refresh_preserving_worker_recovery(
        self: &Arc<Self>,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        if !self.automatic_discovery_allowed().await? {
            sync_service_preferences(self).await?;
            let mut snapshot = self.snapshot();
            snapshot.discovery_pending = false;
            return Ok(snapshot);
        }
        self.refresh_with_worker_recovery(WorkerRecoveryPolicy::Preserve)
            .await
    }

    async fn refresh_after_source_change(
        self: &Arc<Self>,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        if self.profile == ExternalSourceServiceProfile::DiscoveryCatalog {
            self.refresh_preserving_worker_recovery().await
        } else {
            self.refresh().await
        }
    }

    async fn refresh_worker_loss_once(
        self: &Arc<Self>,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        self.refresh_with_worker_recovery(WorkerRecoveryPolicy::PendingOnce)
            .await
    }

    async fn refresh_with_worker_recovery(
        self: &Arc<Self>,
        recovery_policy: WorkerRecoveryPolicy,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        // Preferences are global to the local execution domain and may be
        // changed by another OpenBitFun process. Synchronize before every refresh
        // so a cached CLI/Desktop service cannot keep an externally disabled
        // source active.
        sync_service_preferences(self).await?;
        let _refresh_guard = self.refresh_gate.lock().await;
        let preferences = read_external_sources_config().await?;
        // Recheck after acquiring the gate: a queued watcher request may have
        // been paused while another scan held it. Explicit refresh still runs.
        if self.profile == ExternalSourceServiceProfile::DiscoveryCatalog
            && matches!(recovery_policy, WorkerRecoveryPolicy::Preserve)
            && !self.automatic_discovery_allowed().await?
        {
            return Ok(self.snapshot());
        }
        let policy = self.discovery_policy(&preferences)?;
        if self.profile == ExternalSourceServiceProfile::LocalExecution
            && matches!(recovery_policy, WorkerRecoveryPolicy::ResetAndAttempt)
        {
            reset_external_tool_workspace_recovery_budget(self.workspace_id.as_deref()).await;
        }
        let recovery_targets = if self.profile == ExternalSourceServiceProfile::LocalExecution
            && matches!(
                recovery_policy,
                WorkerRecoveryPolicy::PendingOnce | WorkerRecoveryPolicy::ResetAndAttempt
            ) {
            begin_external_tool_workspace_recovery(self.workspace_id.as_deref()).await
        } else {
            BTreeSet::new()
        };
        let mut requests = Vec::new();
        let mut disabled_command_results = Vec::new();
        for request in lock_coordinator(&self.control_plane).discovery_requests() {
            if integration_capability_is_discoverable(
                &policy,
                request.ecosystem_id().as_str(),
                EXTERNAL_CAPABILITY_COMMAND,
            ) {
                requests.push(request);
            } else {
                disabled_command_results.push(request.disabled());
            }
        }
        let mut tool_requests = Vec::new();
        let mut disabled_tool_results = Vec::new();
        for request in lock_tool_coordinator(&self.control_plane).discovery_requests() {
            if integration_capability_is_discoverable(
                &policy,
                request.ecosystem_id().as_str(),
                EXTERNAL_CAPABILITY_TOOL,
            ) {
                tool_requests.push(request);
            } else {
                disabled_tool_results.push(request.disabled());
            }
        }
        let mut subagent_requests = Vec::new();
        let mut disabled_subagent_results = Vec::new();
        for request in lock_subagent_coordinator(&self.control_plane).discovery_requests() {
            if integration_capability_is_discoverable(
                &policy,
                request.ecosystem_id().as_str(),
                EXTERNAL_CAPABILITY_SUBAGENT,
            ) {
                subagent_requests.push(request);
            } else {
                disabled_subagent_results.push(request.disabled());
            }
        }
        let mut mcp_requests = Vec::new();
        let mut disabled_mcp_results = Vec::new();
        for request in lock_mcp_coordinator(&self.control_plane).discovery_requests() {
            if integration_capability_is_discoverable(
                &policy,
                request.ecosystem_id().as_str(),
                EXTERNAL_CAPABILITY_MCP,
            ) {
                mcp_requests.push(request);
            } else {
                disabled_mcp_results.push(request.disabled());
            }
        }
        let mut workspace_reference_requests = Vec::new();
        let mut disabled_workspace_reference_results = Vec::new();
        for request in
            lock_workspace_reference_coordinator(&self.control_plane).discovery_requests()
        {
            if integration_capability_is_discoverable(
                &policy,
                request.ecosystem_id().as_str(),
                EXTERNAL_CAPABILITY_REFERENCE,
            ) {
                workspace_reference_requests.push(request);
            } else {
                disabled_workspace_reference_results.push(request.disabled());
            }
        }
        let (command_batch, tool_batch, subagent_batch, mcp_batch, workspace_reference_batch) = tokio::join!(
            self.control_plane
                .discover_commands(requests, PROVIDER_DISCOVERY_TIMEOUT),
            self.control_plane
                .discover_tools(tool_requests, PROVIDER_DISCOVERY_TIMEOUT),
            self.control_plane
                .discover_subagents(subagent_requests, PROVIDER_DISCOVERY_TIMEOUT),
            self.control_plane
                .discover_mcp(mcp_requests, PROVIDER_DISCOVERY_TIMEOUT),
            self.control_plane.discover_workspace_references(
                workspace_reference_requests,
                PROVIDER_DISCOVERY_TIMEOUT,
            ),
        );
        let mut results = command_batch.immediate;
        results.append(&mut disabled_command_results);
        let mut tool_results = tool_batch.immediate;
        tool_results.append(&mut disabled_tool_results);
        let mut subagent_results = subagent_batch.immediate;
        subagent_results.append(&mut disabled_subagent_results);
        let mut mcp_results = mcp_batch.immediate;
        mcp_results.append(&mut disabled_mcp_results);
        let mut workspace_reference_results = workspace_reference_batch.immediate;
        workspace_reference_results.append(&mut disabled_workspace_reference_results);
        let command_snapshot =
            lock_coordinator(&self.control_plane).apply_discovery_results(results);
        lock_tool_coordinator(&self.control_plane).apply_discovery_results(tool_results);
        let subagent_snapshot = lock_subagent_coordinator(&self.control_plane)
            .apply_discovery_results(subagent_results);
        lock_mcp_coordinator(&self.control_plane).apply_discovery_results(mcp_results);
        lock_workspace_reference_coordinator(&self.control_plane)
            .apply_discovery_results(workspace_reference_results);
        for deferred in command_batch.deferred {
            self.schedule_deferred_command_discovery(deferred);
        }
        for deferred in tool_batch.deferred {
            self.schedule_deferred_tool_discovery(deferred);
        }
        for deferred in subagent_batch.deferred {
            self.schedule_deferred_subagent_discovery(deferred);
        }
        for deferred in mcp_batch.deferred {
            self.schedule_deferred_mcp_discovery(deferred);
        }
        for deferred in workspace_reference_batch.deferred {
            self.schedule_deferred_workspace_reference_discovery(deferred);
        }
        self.schedule_subagent_last_valid_expiry(&subagent_snapshot);
        self.ensure_watch_roots(&policy).await;
        let snapshot = self
            .rebuild_product_snapshot_with_worker_recovery(command_snapshot, &recovery_targets)
            .await;
        let snapshot = snapshot?;
        let _ = self.updates.send(snapshot.clone());
        self.initial_refresh_completed
            .store(true, Ordering::Release);
        Ok(snapshot)
    }

    async fn ensure_initial_refresh_with<F, Fut>(
        &self,
        refresh: F,
    ) -> Result<ExternalSourceCatalogSnapshot, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<ExternalSourceCatalogSnapshot, String>>,
    {
        if self.initial_refresh_completed.load(Ordering::Acquire) {
            return Ok(self.snapshot());
        }
        let _initial_refresh_guard = self.initial_refresh_gate.lock().await;
        if self.initial_refresh_completed.load(Ordering::Acquire) {
            return Ok(self.snapshot());
        }
        let snapshot = refresh().await?;
        self.initial_refresh_completed
            .store(true, Ordering::Release);
        Ok(snapshot)
    }

    async fn ensure_initial_refresh(
        self: &Arc<Self>,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        if !self.automatic_discovery_allowed().await? {
            let mut snapshot = self.snapshot();
            snapshot.discovery_pending = false;
            return Ok(snapshot);
        }
        self.ensure_initial_refresh_with(|| self.refresh()).await
    }

    async fn rebuild_product_snapshot(
        &self,
        command_snapshot: ExternalSourceCatalogSnapshot,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        self.rebuild_product_snapshot_with_worker_recovery(command_snapshot, &BTreeSet::new())
            .await
    }

    async fn rebuild_product_snapshot_with_worker_recovery(
        &self,
        _command_snapshot: ExternalSourceCatalogSnapshot,
        worker_recovery_targets: &BTreeSet<String>,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let _rebuild_guard = self.product_rebuild_gate.lock().await;
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        let mut preferences = read_external_sources_config().await?;
        let mut policy = integration_policy_snapshot(&preferences, self.workspace_id.as_deref())?;
        if self.profile != ExternalSourceServiceProfile::LocalExecution {
            return self
                .rebuild_read_only_projection(command_snapshot, preferences, policy)
                .await;
        }
        let command_discoverable =
            !ecosystems_with_discoverable_capability(&policy, EXTERNAL_CAPABILITY_COMMAND)
                .is_empty();
        let command_active_ecosystems =
            ecosystems_with_active_capability(&policy, EXTERNAL_CAPABILITY_COMMAND);
        let safe_mode = self.safe_mode_enabled();
        let tool_discoverable =
            !ecosystems_with_discoverable_capability(&policy, EXTERNAL_CAPABILITY_TOOL).is_empty();
        let tool_active_ecosystems = if safe_mode {
            BTreeSet::new()
        } else {
            ecosystems_with_active_capability(&policy, EXTERNAL_CAPABILITY_TOOL)
        };
        let subagent_discoverable =
            !ecosystems_with_discoverable_capability(&policy, EXTERNAL_CAPABILITY_SUBAGENT)
                .is_empty();
        let subagent_active_ecosystems = if safe_mode {
            BTreeSet::new()
        } else {
            ecosystems_with_active_capability(&policy, EXTERNAL_CAPABILITY_SUBAGENT)
        };
        let mcp_discoverable =
            !ecosystems_with_discoverable_capability(&policy, EXTERNAL_CAPABILITY_MCP).is_empty();
        let mcp_active_ecosystems = if safe_mode {
            BTreeSet::new()
        } else {
            ecosystems_with_active_capability(&policy, EXTERNAL_CAPABILITY_MCP)
        };
        let mut state = reconcile_external_tools(
            self.workspace_id.as_deref(),
            self.execution_domain_id.as_str(),
            &self.control_plane,
            ExternalToolDecisions {
                active_ecosystems: &tool_active_ecosystems,
                approved_targets: &preferences.approved_tool_targets,
                declined_decisions_by_approval: &preferences.declined_tool_decisions,
                conflict_choices: &preferences.tool_conflict_choices,
            },
            worker_recovery_targets,
        )
        .await;
        if let Err(error) = persist_observed_tool_conflicts(&state.conflicts).await {
            state.diagnostics.push(ExternalSourceDiagnostic {
                severity: openbitfun_product_domains::external_sources::ExternalSourceDiagnosticSeverity::Warning,
                asset_kind: openbitfun_product_domains::external_sources::ExternalSourceAssetKind::Tool,
                code: "external_tool.conflict_history_write_failed".to_string(),
                message: format!(
                    "Could not persist external tool conflict history; the current catalog remains fail-closed: {error}"
                ),
                source: None,
            });
        }
        let tool_snapshot = lock_tool_coordinator(&self.control_plane).snapshot();
        let mut snapshot = merge_tool_state(command_snapshot, &tool_snapshot, state);
        let mcp_snapshot = lock_mcp_coordinator(&self.control_plane).snapshot();
        let mcp_workspace_key = workspace_route_key(self.workspace_id.as_deref());
        let native_mcp_candidates = if !mcp_active_ecosystems.is_empty() {
            load_native_mcp_candidates(&self.mcp_revision_key).await
        } else {
            Ok(Vec::new())
        };
        let mut mcp_state = match native_mcp_candidates {
            Ok(native_candidates) => reconcile_external_mcp_catalog(
                self.execution_domain_id.as_str(),
                &mcp_workspace_key,
                &mcp_snapshot,
                &native_candidates,
                ExternalMcpDecisions {
                    active_ecosystems: &mcp_active_ecosystems,
                    server_decisions: &preferences.mcp_server_decisions,
                    conflict_choices: &preferences.mcp_conflict_choices,
                },
            ),
            Err(_) => {
                let mut state = reconcile_external_mcp_catalog(
                    self.execution_domain_id.as_str(),
                    &mcp_workspace_key,
                    &mcp_snapshot,
                    &[],
                    ExternalMcpDecisions {
                        active_ecosystems: &mcp_active_ecosystems,
                        server_decisions: &preferences.mcp_server_decisions,
                        conflict_choices: &preferences.mcp_conflict_choices,
                    },
                );
                for entry in &mut state.entries {
                    entry.runtime_id = None;
                    entry.activation_state = ExternalMcpActivationState::RuntimeUnavailable {
                        reason: "external_mcp.native_configuration_unavailable".to_string(),
                    };
                }
                state.active.clear();
                state
            }
        };
        self.reconcile_mcp_runtime(&mut mcp_state).await;
        merge_mcp_state(&mut snapshot, &mcp_snapshot, mcp_state);
        let subagent_snapshot = lock_subagent_coordinator(&self.control_plane).snapshot();
        let mut subagent_state = reconcile_external_subagents(
            self.workspace_id.as_deref(),
            self.workspace_root.as_deref(),
            self.execution_domain_id.as_str(),
            &subagent_snapshot,
            ExternalSubagentDecisions {
                active_ecosystems: &subagent_active_ecosystems,
                approved_envelopes: &preferences.approved_subagent_envelopes,
                declined_decisions: &preferences.declined_subagent_decisions,
                conflict_choices: &preferences.subagent_conflict_choices,
                conflict_lineage_current_keys: &preferences.subagent_conflict_lineage_current_keys,
                model_bindings: &preferences.subagent_model_bindings,
            },
        )
        .await;
        {
            match persist_observed_subagent_conflicts(
                &subagent_state.observed_conflict_lineage_current_keys,
            )
            .await
            {
                Ok((_history_changed, authoritative)) => {
                    let decisions_changed = authoritative.preference_revision
                        != preferences.preference_revision
                        || authoritative.approved_subagent_envelopes
                            != preferences.approved_subagent_envelopes
                        || authoritative.declined_subagent_decisions
                            != preferences.declined_subagent_decisions
                        || authoritative.subagent_conflict_choices
                            != preferences.subagent_conflict_choices
                        || authoritative.subagent_conflict_lineage_current_keys
                            != preferences.subagent_conflict_lineage_current_keys;
                    preferences = authoritative;
                    if decisions_changed {
                        subagent_state = reconcile_external_subagents(
                            self.workspace_id.as_deref(),
                            self.workspace_root.as_deref(),
                            self.execution_domain_id.as_str(),
                            &subagent_snapshot,
                            ExternalSubagentDecisions {
                                active_ecosystems: &subagent_active_ecosystems,
                                approved_envelopes: &preferences.approved_subagent_envelopes,
                                declined_decisions: &preferences.declined_subagent_decisions,
                                conflict_choices: &preferences.subagent_conflict_choices,
                                conflict_lineage_current_keys: &preferences
                                    .subagent_conflict_lineage_current_keys,
                                model_bindings: &preferences.subagent_model_bindings,
                            },
                        )
                        .await;
                    }
                }
                Err(error) => {
                    snapshot.diagnostics.push(ExternalSourceDiagnostic::warning(
                    "external_subagent.conflict_history_write_failed",
                    format!(
                        "Could not persist external subagent conflict history; routes remain unavailable: {error}"
                    ),
                    None,
                ).with_asset_kind(ExternalSourceAssetKind::Subagent));
                }
            }
        }
        merge_subagent_state(
            &mut snapshot,
            &subagent_snapshot,
            &subagent_state,
            preferences.preference_revision,
        );
        let active_subagents = subagent_state
            .registrations
            .iter()
            .map(|registration| {
                (
                    registration.ecosystem_id.clone(),
                    registration.logical_id.to_ascii_lowercase(),
                )
            })
            .collect::<BTreeSet<_>>();
        restrict_prompt_commands_without_active_subagents(
            &mut snapshot.commands,
            &mut snapshot.command_conflicts,
            &active_subagents,
        );
        if let Some(workspace_id) = self.workspace_id.as_deref() {
            crate::agentic::agents::get_agent_registry().install_external_subagent_routes(
                workspace_id,
                subagent_state.registrations,
                subagent_state.routes,
            );
        }
        let source_ecosystems = snapshot
            .sources
            .iter()
            .map(|source| {
                (
                    source.record.key.clone(),
                    source.record.ecosystem_id.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let restricted = PromptCommandAvailability::Restricted {
            reason: "External command execution is disabled by integration policy".to_string(),
            required_capabilities: vec![EXTERNAL_CAPABILITY_COMMAND.to_string()],
        };
        for command in &mut snapshot.commands {
            if !source_ecosystems
                .get(&command.definition.id.source)
                .is_some_and(|ecosystem| command_active_ecosystems.contains(ecosystem))
            {
                command.definition.availability = restricted.clone();
            }
        }
        for conflict in &mut snapshot.command_conflicts {
            for candidate in &mut conflict.candidates {
                if !command_active_ecosystems.contains(&candidate.ecosystem_id) {
                    candidate.availability = restricted.clone();
                    if conflict.selected_candidate_id.as_deref()
                        == Some(candidate.candidate_id.as_str())
                    {
                        conflict.selected_candidate_id = None;
                    }
                }
            }
        }
        if !tool_discoverable {
            snapshot.tools.clear();
            snapshot.tool_approval_requests.clear();
            snapshot.tool_conflicts.clear();
        }
        if !subagent_discoverable {
            snapshot.subagents.clear();
            snapshot.subagent_conflicts.clear();
            snapshot.pending_subagent_approvals.clear();
        }
        if !mcp_discoverable {
            snapshot.mcp_servers.clear();
            snapshot.mcp_approval_requests.clear();
            snapshot.mcp_conflicts.clear();
        }
        if !command_discoverable
            && !tool_discoverable
            && !subagent_discoverable
            && !mcp_discoverable
        {
            snapshot.sources.clear();
            snapshot.diagnostics.clear();
        }
        policy = integration_policy_snapshot(&preferences, self.workspace_id.as_deref())?;
        snapshot.integration_policy = policy;
        assign_external_source_presentation_groups(&mut snapshot);
        sanitize_external_snapshot_locations(&mut snapshot, self.workspace_root.as_deref());
        let mut current = lock_snapshot(&self.snapshot);
        let mcp_changed = snapshot.mcp_servers != current.mcp_servers
            || snapshot.mcp_conflicts != current.mcp_conflicts
            || snapshot.mcp_approval_requests != current.mcp_approval_requests;
        snapshot.mcp_generation = if mcp_changed {
            snapshot
                .mcp_generation
                .max(current.mcp_generation.saturating_add(1))
        } else {
            snapshot.mcp_generation.max(current.mcp_generation)
        };
        let subagent_changed = snapshot.subagents != current.subagents
            || snapshot.subagent_conflicts != current.subagent_conflicts
            || snapshot.pending_subagent_approvals != current.pending_subagent_approvals
            || snapshot.preference_revision != current.preference_revision;
        snapshot.subagent_generation = if subagent_changed {
            snapshot
                .subagent_generation
                .max(current.subagent_generation.saturating_add(1))
        } else {
            snapshot
                .subagent_generation
                .max(current.subagent_generation)
        };
        snapshot.generation = snapshot
            .generation
            .max(current.generation.saturating_add(1));
        *current = snapshot.clone();
        Ok(snapshot)
    }

    async fn rebuild_read_only_projection(
        &self,
        command_snapshot: ExternalSourceCatalogSnapshot,
        preferences: ExternalSourcesConfig,
        policy: ExternalIntegrationPolicySnapshot,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let tool_active_ecosystems =
            ecosystems_with_active_capability(&policy, EXTERNAL_CAPABILITY_TOOL);
        let subagent_active_ecosystems =
            ecosystems_with_active_capability(&policy, EXTERNAL_CAPABILITY_SUBAGENT);
        let mcp_active_ecosystems =
            ecosystems_with_active_capability(&policy, EXTERNAL_CAPABILITY_MCP);

        let tool_snapshot = lock_tool_coordinator(&self.control_plane).snapshot();
        let tool_state = project_external_tools_read_only(
            self.execution_domain_id.as_str(),
            &tool_snapshot,
            ExternalToolDecisions {
                active_ecosystems: &tool_active_ecosystems,
                approved_targets: &preferences.approved_tool_targets,
                declined_decisions_by_approval: &preferences.declined_tool_decisions,
                conflict_choices: &preferences.tool_conflict_choices,
            },
        );
        let mut snapshot = merge_tool_state(command_snapshot, &tool_snapshot, tool_state);

        let mcp_snapshot = lock_mcp_coordinator(&self.control_plane).snapshot();
        let mcp_workspace_key = workspace_route_key(self.workspace_id.as_deref());
        let mut mcp_state = reconcile_external_mcp_catalog(
            self.execution_domain_id.as_str(),
            &mcp_workspace_key,
            &mcp_snapshot,
            &[],
            ExternalMcpDecisions {
                active_ecosystems: &mcp_active_ecosystems,
                server_decisions: &preferences.mcp_server_decisions,
                conflict_choices: &preferences.mcp_conflict_choices,
            },
        );
        mcp_state.active.clear();
        mcp_state.suppressed_native_server_ids.clear();
        for entry in &mut mcp_state.entries {
            entry.runtime_id = None;
            if matches!(
                entry.activation_state,
                ExternalMcpActivationState::Active | ExternalMcpActivationState::Starting
            ) {
                entry.activation_state = ExternalMcpActivationState::RuntimeUnavailable {
                    reason: "external_mcp.runtime.host_read_only".to_string(),
                };
            }
        }
        merge_mcp_state(&mut snapshot, &mcp_snapshot, mcp_state);

        let subagent_snapshot = lock_subagent_coordinator(&self.control_plane).snapshot();
        let subagent_state = project_external_subagents_read_only(
            self.workspace_id.as_deref(),
            self.workspace_root.as_deref(),
            self.execution_domain_id.as_str(),
            &subagent_snapshot,
            ExternalSubagentDecisions {
                active_ecosystems: &subagent_active_ecosystems,
                approved_envelopes: &preferences.approved_subagent_envelopes,
                declined_decisions: &preferences.declined_subagent_decisions,
                conflict_choices: &preferences.subagent_conflict_choices,
                conflict_lineage_current_keys: &preferences.subagent_conflict_lineage_current_keys,
                model_bindings: &preferences.subagent_model_bindings,
            },
        );
        merge_subagent_state(
            &mut snapshot,
            &subagent_snapshot,
            &subagent_state,
            preferences.preference_revision,
        );

        let restricted = PromptCommandAvailability::Restricted {
            reason: "This Host exposes discovery only; run external commands from Desktop or an authenticated Peer Host".to_string(),
            required_capabilities: vec![EXTERNAL_CAPABILITY_COMMAND.to_string()],
        };
        for command in &mut snapshot.commands {
            command.definition.availability = restricted.clone();
        }
        for conflict in &mut snapshot.command_conflicts {
            conflict.selected_candidate_id = None;
            for candidate in &mut conflict.candidates {
                candidate.availability = restricted.clone();
            }
        }
        snapshot.integration_policy = policy;
        assign_external_source_presentation_groups(&mut snapshot);
        sanitize_external_snapshot_locations(&mut snapshot, self.workspace_root.as_deref());
        let mut current = lock_snapshot(&self.snapshot);
        let mcp_changed = snapshot.mcp_servers != current.mcp_servers
            || snapshot.mcp_conflicts != current.mcp_conflicts
            || snapshot.mcp_approval_requests != current.mcp_approval_requests;
        snapshot.mcp_generation = if mcp_changed {
            snapshot
                .mcp_generation
                .max(current.mcp_generation.saturating_add(1))
        } else {
            snapshot.mcp_generation.max(current.mcp_generation)
        };
        let subagent_changed = snapshot.subagents != current.subagents
            || snapshot.subagent_conflicts != current.subagent_conflicts
            || snapshot.pending_subagent_approvals != current.pending_subagent_approvals
            || snapshot.preference_revision != current.preference_revision;
        snapshot.subagent_generation = if subagent_changed {
            snapshot
                .subagent_generation
                .max(current.subagent_generation.saturating_add(1))
        } else {
            snapshot
                .subagent_generation
                .max(current.subagent_generation)
        };
        snapshot.generation = snapshot
            .generation
            .max(current.generation.saturating_add(1));
        *current = snapshot.clone();
        Ok(snapshot)
    }

    async fn reconcile_mcp_runtime(&self, state: &mut ExternalMcpProductState) {
        let desired = state
            .active
            .iter()
            .map(|candidate| (candidate.runtime_id.clone(), candidate.clone()))
            .collect::<BTreeMap<_, _>>();
        let desired_ids = desired.keys().cloned().collect::<BTreeSet<_>>();
        let mut managed = self.active_mcp_runtime_ids.lock().await.clone();
        let workspace_key = workspace_route_key(self.workspace_id.as_deref());
        if let Err(reason) = self
            .mcp_runtime
            .replace_workspace_route(
                &workspace_key,
                desired_ids.clone(),
                state.suppressed_native_server_ids.clone(),
            )
            .await
        {
            state.diagnostics.push(
                ExternalSourceDiagnostic::warning("external_mcp.route_update_failed", reason, None)
                    .with_asset_kind(ExternalSourceAssetKind::Mcp),
            );
        }
        let managed_statuses = join_all(
            desired
                .values()
                .filter(|candidate| managed.contains(&candidate.runtime_id))
                .map(|candidate| async {
                    (
                        candidate.runtime_id.clone(),
                        self.mcp_runtime.status(&candidate.runtime_id).await,
                    )
                }),
        )
        .await
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        for runtime_id in managed
            .difference(&desired_ids)
            .cloned()
            .collect::<Vec<_>>()
        {
            match self.mcp_runtime.retire(&runtime_id).await {
                Ok(()) => {
                    managed.remove(&runtime_id);
                }
                Err(reason) => state.diagnostics.push(
                    ExternalSourceDiagnostic::warning(
                        "external_mcp.retirement_failed",
                        reason,
                        None,
                    )
                    .with_asset_kind(ExternalSourceAssetKind::Mcp),
                ),
            }
        }

        for candidate in desired.values() {
            if managed.contains(&candidate.runtime_id) {
                let status = managed_statuses
                    .get(&candidate.runtime_id)
                    .cloned()
                    .unwrap_or_else(|| Err(EXTERNAL_MCP_RUNTIME_STATUS_UNAVAILABLE.to_string()));
                // Keep failed registrations managed until the user disables
                // them. Re-installing from a status error would turn a
                // persistent startup failure into an unbounded retry loop.
                apply_external_mcp_runtime_status(state, candidate, status);
                continue;
            }

            let coordinator = Arc::clone(&self.control_plane);
            let server_id = candidate.definition.id.clone();
            let behavior_version = candidate.definition.behavior_version.clone();
            let prepared = tokio::task::spawn_blocking(move || {
                lock_mcp_coordinator(&coordinator)
                    .prepare_server_guarded(&server_id, &behavior_version)
            })
            .await
            .map_err(|_| EXTERNAL_MCP_RUNTIME_PREPARATION_FAILED.to_string())
            .and_then(|result| {
                result.map_err(|_| EXTERNAL_MCP_RUNTIME_PREPARATION_FAILED.to_string())
            });

            let activation = match prepared {
                Ok(prepared) => {
                    self.mcp_runtime
                        .install(candidate, prepared, &workspace_key)
                        .await
                }
                Err(reason) => Err(reason),
            };
            match activation {
                Ok(()) => {
                    managed.insert(candidate.runtime_id.clone());
                    apply_external_mcp_runtime_status(
                        state,
                        candidate,
                        self.mcp_runtime.status(&candidate.runtime_id).await,
                    );
                }
                Err(reason) => {
                    // A process may have reached a failed-but-registered state.
                    // Track it so later source changes retire it safely instead
                    // of attempting duplicate installs on every refresh.
                    if self.mcp_runtime.status(&candidate.runtime_id).await.is_ok() {
                        managed.insert(candidate.runtime_id.clone());
                    }
                    mark_external_mcp_runtime_unavailable(state, candidate, reason);
                }
            }
        }

        *self.active_mcp_runtime_ids.lock().await = managed;
    }

    fn schedule_deferred_command_discovery(
        self: &Arc<Self>,
        deferred: DeferredDiscovery<ExternalSourceDiscoveryResult>,
    ) {
        let weak = Arc::downgrade(self);
        let control_plane = Arc::clone(&self.control_plane);
        tokio::spawn(async move {
            let mut deferred = deferred;
            loop {
                let Some((completed, observer)) = control_plane.complete_command(deferred).await
                else {
                    return;
                };
                {
                    let Some(service) = weak.upgrade() else {
                        return;
                    };
                    let _refresh_guard = service.refresh_gate.lock().await;
                    let Some(result) = control_plane.finalize_command(completed).await else {
                        return;
                    };
                    service.complete_deferred_discovery(result).await;
                }
                let Some(observer) = observer else {
                    return;
                };
                let Some(next) = control_plane.resume_abandoned_command(observer).await else {
                    return;
                };
                deferred = next;
            }
        });
    }

    fn schedule_deferred_tool_discovery(
        self: &Arc<Self>,
        deferred: DeferredDiscovery<ExternalToolDiscoveryResult>,
    ) {
        let weak = Arc::downgrade(self);
        let control_plane = Arc::clone(&self.control_plane);
        tokio::spawn(async move {
            let mut deferred = deferred;
            loop {
                let Some((completed, observer)) = control_plane.complete_tool(deferred).await
                else {
                    return;
                };
                {
                    let Some(service) = weak.upgrade() else {
                        return;
                    };
                    let _refresh_guard = service.refresh_gate.lock().await;
                    let Some(result) = control_plane.finalize_tool(completed).await else {
                        return;
                    };
                    service.complete_deferred_tool_discovery(result).await;
                }
                let Some(observer) = observer else {
                    return;
                };
                let Some(next) = control_plane.resume_abandoned_tool(observer).await else {
                    return;
                };
                deferred = next;
            }
        });
    }

    fn schedule_deferred_subagent_discovery(
        self: &Arc<Self>,
        deferred: DeferredDiscovery<ExternalSubagentDiscoveryResult>,
    ) {
        let weak = Arc::downgrade(self);
        let control_plane = Arc::clone(&self.control_plane);
        tokio::spawn(async move {
            let mut deferred = deferred;
            loop {
                let Some((completed, observer)) = control_plane.complete_subagent(deferred).await
                else {
                    return;
                };
                {
                    let Some(service) = weak.upgrade() else {
                        return;
                    };
                    let _refresh_guard = service.refresh_gate.lock().await;
                    let Some(result) = control_plane.finalize_subagent(completed).await else {
                        return;
                    };
                    service.complete_deferred_subagent_discovery(result).await;
                }
                let Some(observer) = observer else {
                    return;
                };
                let Some(next) = control_plane.resume_abandoned_subagent(observer).await else {
                    return;
                };
                deferred = next;
            }
        });
    }

    fn schedule_deferred_mcp_discovery(
        self: &Arc<Self>,
        deferred: DeferredDiscovery<ExternalMcpDiscoveryResult>,
    ) {
        let weak = Arc::downgrade(self);
        let control_plane = Arc::clone(&self.control_plane);
        tokio::spawn(async move {
            let mut deferred = deferred;
            loop {
                let Some((completed, observer)) = control_plane.complete_mcp(deferred).await else {
                    return;
                };
                {
                    let Some(service) = weak.upgrade() else {
                        return;
                    };
                    let _refresh_guard = service.refresh_gate.lock().await;
                    let Some(result) = control_plane.finalize_mcp(completed).await else {
                        return;
                    };
                    service.complete_deferred_mcp_discovery(result).await;
                }
                let Some(observer) = observer else {
                    return;
                };
                let Some(next) = control_plane.resume_abandoned_mcp(observer).await else {
                    return;
                };
                deferred = next;
            }
        });
    }

    fn schedule_deferred_workspace_reference_discovery(
        self: &Arc<Self>,
        deferred: DeferredDiscovery<ExternalWorkspaceReferenceDiscoveryResult>,
    ) {
        let weak = Arc::downgrade(self);
        let control_plane = Arc::clone(&self.control_plane);
        tokio::spawn(async move {
            let mut deferred = deferred;
            loop {
                let Some((completed, observer)) =
                    control_plane.complete_workspace_reference(deferred).await
                else {
                    return;
                };
                {
                    let Some(service) = weak.upgrade() else {
                        return;
                    };
                    let _refresh_guard = service.refresh_gate.lock().await;
                    let Some(result) = control_plane.finalize_workspace_reference(completed).await
                    else {
                        return;
                    };
                    service
                        .complete_deferred_workspace_reference_discovery(result)
                        .await;
                }
                let Some(observer) = observer else {
                    return;
                };
                let Some(next) = control_plane
                    .resume_abandoned_workspace_reference(observer)
                    .await
                else {
                    return;
                };
                deferred = next;
            }
        });
    }

    async fn complete_deferred_discovery(&self, result: ExternalSourceDiscoveryResult) {
        let provider_id = result.provider_id().clone();
        let Ok(preferences) = read_external_sources_config().await else {
            return;
        };
        let Ok(policy) = self.discovery_policy(&preferences) else {
            return;
        };
        let ecosystem_id =
            lock_coordinator(&self.control_plane).ecosystem_for_provider(&provider_id);
        if ecosystem_id.is_none_or(|ecosystem_id| {
            !integration_capability_is_discoverable(
                &policy,
                ecosystem_id.as_str(),
                EXTERNAL_CAPABILITY_COMMAND,
            )
        }) {
            self.ensure_watch_roots(&policy).await;
            return;
        }
        let command_snapshot = lock_coordinator(&self.control_plane).apply_discovery_result(result);
        self.ensure_watch_roots(&policy).await;
        if let Ok(snapshot) = self.rebuild_product_snapshot(command_snapshot).await {
            let _ = self.updates.send(snapshot);
        }
    }

    async fn complete_deferred_tool_discovery(&self, result: ExternalToolDiscoveryResult) {
        let provider_id = result.provider_id().clone();
        let Ok(preferences) = read_external_sources_config().await else {
            return;
        };
        let Ok(policy) = self.discovery_policy(&preferences) else {
            return;
        };
        let ecosystem_id =
            lock_tool_coordinator(&self.control_plane).ecosystem_for_provider(&provider_id);
        if ecosystem_id.is_none_or(|ecosystem_id| {
            !integration_capability_is_discoverable(
                &policy,
                ecosystem_id.as_str(),
                EXTERNAL_CAPABILITY_TOOL,
            )
        }) {
            self.ensure_watch_roots(&policy).await;
            return;
        }
        lock_tool_coordinator(&self.control_plane).apply_discovery_result(result);
        self.ensure_watch_roots(&policy).await;
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        if let Ok(snapshot) = self.rebuild_product_snapshot(command_snapshot).await {
            let _ = self.updates.send(snapshot);
        }
    }

    async fn complete_deferred_subagent_discovery(
        self: &Arc<Self>,
        result: ExternalSubagentDiscoveryResult,
    ) {
        let provider_id = result.provider_id().clone();
        let Ok(preferences) = read_external_sources_config().await else {
            return;
        };
        let Ok(policy) = self.discovery_policy(&preferences) else {
            return;
        };
        let ecosystem_id =
            lock_subagent_coordinator(&self.control_plane).ecosystem_for_provider(&provider_id);
        if ecosystem_id.is_none_or(|ecosystem_id| {
            !integration_capability_is_discoverable(
                &policy,
                ecosystem_id.as_str(),
                EXTERNAL_CAPABILITY_SUBAGENT,
            )
        }) {
            self.ensure_watch_roots(&policy).await;
            return;
        }
        let subagent_snapshot =
            lock_subagent_coordinator(&self.control_plane).apply_discovery_result(result);
        self.schedule_subagent_last_valid_expiry(&subagent_snapshot);
        self.ensure_watch_roots(&policy).await;
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        if let Ok(snapshot) = self.rebuild_product_snapshot(command_snapshot).await {
            let _ = self.updates.send(snapshot);
        }
    }

    async fn complete_deferred_mcp_discovery(&self, result: ExternalMcpDiscoveryResult) {
        let provider_id = result.provider_id().clone();
        let Ok(preferences) = read_external_sources_config().await else {
            return;
        };
        let Ok(policy) = self.discovery_policy(&preferences) else {
            return;
        };
        let ecosystem_id =
            lock_mcp_coordinator(&self.control_plane).ecosystem_for_provider(&provider_id);
        if ecosystem_id.is_none_or(|ecosystem_id| {
            !integration_capability_is_discoverable(
                &policy,
                ecosystem_id.as_str(),
                EXTERNAL_CAPABILITY_MCP,
            )
        }) {
            self.ensure_watch_roots(&policy).await;
            return;
        }
        lock_mcp_coordinator(&self.control_plane).apply_discovery_result(result);
        self.ensure_watch_roots(&policy).await;
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        if let Ok(snapshot) = self.rebuild_product_snapshot(command_snapshot).await {
            let _ = self.updates.send(snapshot);
        }
    }

    async fn complete_deferred_workspace_reference_discovery(
        &self,
        result: ExternalWorkspaceReferenceDiscoveryResult,
    ) {
        let provider_id = result.provider_id().clone();
        let Ok(preferences) = read_external_sources_config().await else {
            return;
        };
        let Ok(policy) = self.discovery_policy(&preferences) else {
            return;
        };
        let ecosystem_id = lock_workspace_reference_coordinator(&self.control_plane)
            .ecosystem_for_provider(&provider_id);
        if ecosystem_id.is_none_or(|ecosystem_id| {
            !integration_capability_is_discoverable(
                &policy,
                ecosystem_id.as_str(),
                EXTERNAL_CAPABILITY_REFERENCE,
            )
        }) {
            self.ensure_watch_roots(&policy).await;
            return;
        }
        lock_workspace_reference_coordinator(&self.control_plane).apply_discovery_result(result);
        self.ensure_watch_roots(&policy).await;
    }

    fn schedule_subagent_last_valid_expiry(
        self: &Arc<Self>,
        snapshot: &openbitfun_external_sources::ExternalSubagentCoordinatorSnapshot,
    ) {
        let schedule = self
            .subagent_expiry_schedule
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let Some(deadline) = snapshot.next_refresh_deadline else {
            return;
        };
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
            let Some(service) = weak.upgrade() else {
                return;
            };
            if service.subagent_expiry_schedule.load(Ordering::Acquire) != schedule {
                return;
            }
            let _refresh_guard = service.refresh_gate.lock().await;
            lock_subagent_coordinator(&service.control_plane).expire_last_valid();
            let command_snapshot = lock_coordinator(&service.control_plane).snapshot();
            if let Ok(snapshot) = service.rebuild_product_snapshot(command_snapshot).await {
                let _ = service.updates.send(snapshot);
            }
        });
    }

    fn ensure_background_refresh(self: &Arc<Self>) {
        if self.initial_refresh_completed.load(Ordering::Acquire)
            || self
                .background_refresh_scheduled
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let Some(service) = weak.upgrade() else {
                return;
            };
            if let Err(error) = service.ensure_initial_refresh().await {
                log::warn!(
                    "Initial external source refresh failed scope={} error_category={}",
                    external_log_scope(service.workspace_root.as_deref()),
                    external_log_error_category(&error),
                );
            }
            service
                .background_refresh_scheduled
                .store(false, Ordering::Release);
        });
    }

    fn touch(&self) {
        self.last_access_epoch_seconds
            .store(epoch_seconds(), Ordering::Release);
    }

    fn ensure_idle_keepalive(self: &Arc<Self>) {
        if self
            .keepalive_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let service = Arc::clone(self);
        tokio::spawn(async move {
            const IDLE_SECONDS: u64 = 300;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                let idle_for = epoch_seconds()
                    .saturating_sub(service.last_access_epoch_seconds.load(Ordering::Acquire));
                // The keepalive itself and this task account for one strong
                // service reference. A subscription or in-flight operation
                // keeps the service alive independently of idle time.
                if idle_for < IDLE_SECONDS || Arc::strong_count(&service) > 1 {
                    continue;
                }
                let _service_gate = workspace_service_gate().lock().await;
                let idle_for = epoch_seconds()
                    .saturating_sub(service.last_access_epoch_seconds.load(Ordering::Acquire));
                if idle_for < IDLE_SECONDS || Arc::strong_count(&service) > 1 {
                    continue;
                }
                let _rebuild_guard = service.product_rebuild_gate.lock().await;
                if Arc::strong_count(&service) > 1 {
                    continue;
                }
                let key = service.workspace_id.clone();
                let services = workspace_services_for_profile(service.profile);
                if let Some(entry) = services.get(&key) {
                    let should_remove = entry
                        .value()
                        .upgrade()
                        .is_some_and(|cached| Arc::ptr_eq(&cached, &service));
                    drop(entry);
                    if should_remove {
                        if service.profile == ExternalSourceServiceProfile::LocalExecution {
                            let runtime_ids =
                                std::mem::take(&mut *service.active_mcp_runtime_ids.lock().await);
                            let workspace_key = workspace_route_key(key.as_deref());
                            let _ = service
                                .mcp_runtime
                                .replace_workspace_route(
                                    &workspace_key,
                                    BTreeSet::new(),
                                    BTreeSet::new(),
                                )
                                .await;
                            for runtime_id in runtime_ids {
                                if let Err(error) = service.mcp_runtime.retire(&runtime_id).await {
                                    log::warn!(
                                        "Could not retire idle external MCP runtime runtime_id={} error_category={}",
                                        safe_external_log_token(&runtime_id),
                                        external_log_error_category(&error.to_string()),
                                    );
                                }
                            }
                        }
                        services.remove(&key);
                        if service.profile == ExternalSourceServiceProfile::LocalExecution {
                            release_external_tool_workspace(key.as_deref()).await;
                            if let Some(workspace_id) = service.workspace_id.as_deref() {
                                crate::agentic::agents::get_agent_registry()
                                    .release_external_subagent_workspace(workspace_id);
                            }
                        }
                    }
                }
                break;
            }
        });
    }

    fn snapshot(&self) -> ExternalSourceCatalogSnapshot {
        #[cfg(feature = "opencode-plugin-host")]
        let mut snapshot = lock_snapshot(&self.snapshot).clone();
        #[cfg(not(feature = "opencode-plugin-host"))]
        let snapshot = lock_snapshot(&self.snapshot).clone();
        #[cfg(feature = "opencode-plugin-host")]
        if self.profile != ExternalSourceServiceProfile::DiscoveryCatalog {
            for message in crate::plugin_host::configured_plugin_activation_failures(
                self.workspace_id.as_deref(),
            ) {
                if !snapshot.diagnostics.iter().any(|diagnostic| {
                    diagnostic.code == "plugin.activation_failed" && diagnostic.message == message
                }) {
                    snapshot.diagnostics.push(
                        ExternalSourceDiagnostic::warning(
                            "plugin.activation_failed",
                            message,
                            None,
                        )
                        .with_asset_kind(ExternalSourceAssetKind::Hook),
                    );
                }
            }
        }
        snapshot
    }

    fn source_location(&self, stable_key: &str) -> Result<PathBuf, String> {
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        let tool_snapshot = lock_tool_coordinator(&self.control_plane).snapshot();
        let subagent_snapshot = lock_subagent_coordinator(&self.control_plane).snapshot();
        let mcp_snapshot = lock_mcp_coordinator(&self.control_plane).snapshot();
        resolve_external_source_location(
            stable_key,
            command_snapshot
                .sources
                .iter()
                .chain(tool_snapshot.sources.iter())
                .chain(subagent_snapshot.sources.iter())
                .chain(mcp_snapshot.sources.iter()),
        )
    }

    fn surface_snapshot(
        &self,
        host_capabilities: ExternalSourceHostCapabilities,
    ) -> ExternalSourceSurfaceSnapshotV1 {
        let catalog = self.snapshot();
        let safe_mode = self.safe_mode_enabled();
        let control = ExternalSourceControlSnapshotV1::from_catalog(
            &catalog,
            self.execution_domain_id.clone(),
            safe_mode,
            host_capabilities,
        );
        let mut public = ExternalSourcePublicSnapshot::from(catalog);
        public.host_capabilities = host_capabilities;
        ExternalSourceSurfaceSnapshotV1 {
            control,
            catalog: public,
        }
    }

    fn safe_mode_enabled(&self) -> bool {
        external_source_safe_mode_enabled_for(
            self.execution_domain_id.as_str(),
            &workspace_route_key(self.workspace_id.as_deref()),
        )
    }

    fn write_safe_mode(&self, enabled: bool) {
        set_external_source_safe_mode_for(
            self.execution_domain_id.as_str(),
            &workspace_route_key(self.workspace_id.as_deref()),
            enabled,
        );
    }

    async fn set_safe_mode(
        &self,
        enabled: bool,
        expected_preference_revision: Option<u64>,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let _refresh_guard = self.refresh_gate.lock().await;
        let previous = self.safe_mode_enabled();
        if let Some(expected_revision) = expected_preference_revision {
            let (accepted, _) = ExternalSourcePreferenceStore::global()?
                .update(|config| {
                    if config.preference_revision != expected_revision {
                        return false;
                    }
                    // Safe Mode is process-local, but its CAS linearization
                    // point must share the authoritative preference file lock
                    // with every persisted source/review mutation.
                    self.write_safe_mode(enabled);
                    true
                })
                .await?;
            if !accepted {
                return Err(stale_operation_error(
                    "External source preferences changed; refresh before changing Safe Mode",
                ));
            }
            if let Err(error) = sync_service_preferences(self).await {
                self.write_safe_mode(previous);
                return Err(error);
            }
        } else {
            self.write_safe_mode(enabled);
        }
        if previous == enabled {
            return Ok(self.snapshot());
        }

        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        match self.rebuild_product_snapshot(command_snapshot).await {
            Ok(snapshot) => {
                let _ = self.updates.send(snapshot.clone());
                Ok(snapshot)
            }
            Err(error) => {
                self.write_safe_mode(previous);
                let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
                if let Err(rollback_error) = self.rebuild_product_snapshot(command_snapshot).await {
                    log::error!(
                        "External source Safe Mode rollback failed execution_domain={} reason={}",
                        safe_external_log_token(self.execution_domain_id.as_str()),
                        safe_external_log_token(&rollback_error),
                    );
                }
                Err(error)
            }
        }
    }

    async fn apply_control_action(
        self: &Arc<Self>,
        request: ExternalSourceControlRequestV1,
    ) -> ExternalSourceOperationResult<ExternalSourceSurfaceSnapshotV1> {
        use openbitfun_product_domains::external_source_control::ExternalSourceOperationStage;

        if let Err(detail) = request.validate() {
            return Err(ExternalSourceOperationError::invalid_request(detail)
                .with_correlation_id(request.operation_id)
                .with_stage(ExternalSourceOperationStage::ValidateRequest));
        }

        let started = std::time::Instant::now();
        let operation_id = request.operation_id;
        let expected_revision = request.expected_preference_revision;
        let (action, stage, result) = match request.action {
            ExternalSourceControlActionV1::Refresh => (
                "refresh",
                ExternalSourceOperationStage::Discover,
                self.refresh_with_runtime_invalidation().await,
            ),
            ExternalSourceControlActionV1::SetSourceEnabled {
                source_key,
                enabled,
            } => {
                let stage = ExternalSourceOperationStage::ApplyPreference;
                let result = match expected_revision {
                    Some(revision) => {
                        self.set_source_enabled(&source_key, enabled, revision)
                            .await
                    }
                    None => Err(invalid_operation_error(
                        "External source enablement requires an expected preference revision",
                    )),
                };
                ("set_source_enabled", stage, result)
            }
            ExternalSourceControlActionV1::SetSafeMode { enabled } => {
                let stage = ExternalSourceOperationStage::ApplyPreference;
                let result = match expected_revision {
                    Some(revision) => self.set_safe_mode(enabled, Some(revision)).await,
                    None => Err(invalid_operation_error(
                        "Safe Mode requires an expected preference revision",
                    )),
                };
                ("set_safe_mode", stage, result)
            }
        };

        match result {
            Ok(snapshot) => {
                log::info!(
                    "External source control action outcome=success action={} correlation_id={} duration_ms={} generation={}",
                    action,
                    safe_external_log_token(&operation_id),
                    started.elapsed().as_millis(),
                    snapshot.generation,
                );
                Ok(self.surface_snapshot(ExternalSourceHostCapabilities::read_write()))
            }
            Err(error) => {
                let typed = typed_control_operation_error(error, &operation_id, stage);
                log::warn!(
                    "External source control action outcome=failure action={} correlation_id={} duration_ms={} error_code={}",
                    action,
                    safe_external_log_token(&operation_id),
                    started.elapsed().as_millis(),
                    typed.code.as_str(),
                );
                Err(typed)
            }
        }
    }

    async fn set_source_enabled(
        self: &Arc<Self>,
        stable_key: &str,
        enabled: bool,
        expected_preference_revision: u64,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let refresh_guard = self.refresh_gate.lock().await;
        if self.snapshot().preference_revision != expected_preference_revision {
            return Err(stale_operation_error(
                "External source preferences changed; refresh before retrying",
            ));
        }
        let (previous_commands, command_known) = {
            let mut coordinator = lock_coordinator(&self.control_plane);
            let previous = coordinator.suppressed_sources().clone();
            let known = coordinator.set_source_enabled(stable_key, enabled).is_ok();
            (previous, known)
        };
        let (previous_tools, tool_known) = {
            let mut coordinator = lock_tool_coordinator(&self.control_plane);
            let previous = coordinator.suppressed_sources().clone();
            let known = coordinator.set_source_enabled(stable_key, enabled).is_ok();
            (previous, known)
        };
        let (previous_subagents, subagent_known) = {
            let mut coordinator = lock_subagent_coordinator(&self.control_plane);
            let previous = coordinator.suppressed_sources().clone();
            let known = coordinator.set_source_enabled(stable_key, enabled).is_ok();
            (previous, known)
        };
        let (previous_mcps, mcp_known) = {
            let mut coordinator = lock_mcp_coordinator(&self.control_plane);
            let previous = coordinator.suppressed_sources().clone();
            let known = coordinator.set_source_enabled(stable_key, enabled).is_ok();
            (previous, known)
        };
        let (previous_workspace_references, workspace_reference_known) = {
            let mut coordinator = lock_workspace_reference_coordinator(&self.control_plane);
            let previous = coordinator.suppressed_sources().clone();
            let known = coordinator.set_source_enabled(stable_key, enabled).is_ok();
            (previous, known)
        };
        if !command_known
            && !tool_known
            && !subagent_known
            && !mcp_known
            && !workspace_reference_known
        {
            return Err(missing_candidate_error(format!(
                "External source '{stable_key}' is no longer available"
            )));
        }
        let authoritative =
            match persist_source_enabled_change(stable_key, enabled, expected_preference_revision)
                .await
            {
                Ok(authoritative) => authoritative,
                Err(error) => {
                    lock_coordinator(&self.control_plane)
                        .replace_suppressed_sources(previous_commands);
                    lock_tool_coordinator(&self.control_plane)
                        .replace_suppressed_sources(previous_tools);
                    lock_subagent_coordinator(&self.control_plane)
                        .replace_suppressed_sources(previous_subagents);
                    lock_mcp_coordinator(&self.control_plane)
                        .replace_suppressed_sources(previous_mcps);
                    lock_workspace_reference_coordinator(&self.control_plane)
                        .replace_suppressed_sources(previous_workspace_references);
                    return Err(error);
                }
            };
        lock_coordinator(&self.control_plane).replace_suppressed_sources(authoritative.clone());
        lock_tool_coordinator(&self.control_plane)
            .replace_suppressed_sources(authoritative.clone());
        lock_subagent_coordinator(&self.control_plane)
            .replace_suppressed_sources(authoritative.clone());
        lock_mcp_coordinator(&self.control_plane).replace_suppressed_sources(authoritative.clone());
        lock_workspace_reference_coordinator(&self.control_plane)
            .replace_suppressed_sources(authoritative.clone());
        propagate_suppressed_sources(&authoritative, self);
        // Refresh acquires the same gate. Release the mutation critical section
        // after the preference and in-memory coordinators agree, then refresh
        // from the authoritative store to avoid self-deadlocking the request.
        drop(refresh_guard);
        self.refresh_preserving_worker_recovery().await
    }

    async fn update_integration_policy(
        self: &Arc<Self>,
        mutation: ExternalIntegrationPolicyMutation,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let preferences =
            persist_integration_policy_mutation(self.workspace_id.as_deref(), mutation).await?;
        propagate_integration_policy_preferences(&preferences, self);
        self.refresh_preserving_worker_recovery().await
    }

    async fn set_conflict_choice(
        &self,
        conflict_key: &str,
        candidate_id: &str,
        expected_preference_revision: u64,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let _refresh_guard = self.refresh_gate.lock().await;
        let product_snapshot = self.snapshot();
        if product_snapshot.preference_revision != expected_preference_revision {
            return Err(stale_operation_error(
                "External command preferences changed; refresh before retrying",
            ));
        }
        let selected_candidate = product_snapshot
            .command_conflicts
            .iter()
            .find(|conflict| conflict.conflict_key == conflict_key)
            .and_then(|conflict| {
                conflict
                    .candidates
                    .iter()
                    .find(|candidate| candidate.candidate_id == candidate_id)
            })
            .ok_or_else(|| {
                missing_candidate_error(format!(
                    "External source conflict '{conflict_key}' is no longer available"
                ))
            })?;
        if !integration_capability_is_active(
            &product_snapshot.integration_policy,
            selected_candidate.ecosystem_id.as_str(),
            EXTERNAL_CAPABILITY_COMMAND,
        ) {
            return Err(policy_limited_error(
                "The selected external command ecosystem is not enabled for this workspace",
            ));
        }
        let (previous_choices, previous_lineage_keys, previous_conflicted_ids, participants) = {
            let mut coordinator = lock_coordinator(&self.control_plane);
            let participants = coordinator
                .snapshot()
                .command_conflicts
                .into_iter()
                .find(|conflict| conflict.conflict_key == conflict_key)
                .map(|conflict| {
                    conflict
                        .candidates
                        .into_iter()
                        .map(|candidate| candidate.candidate_id)
                        .collect::<Vec<_>>()
                })
                .ok_or_else(|| {
                    missing_candidate_error(format!(
                        "External source conflict '{conflict_key}' is no longer available"
                    ))
                })?;
            let previous_choices = coordinator.conflict_choices().clone();
            let previous_lineage_keys = coordinator.conflict_lineage_current_keys().clone();
            let previous_conflicted_ids = coordinator.conflicted_candidate_ids().clone();
            coordinator.set_conflict_choice(conflict_key, candidate_id)?;
            (
                previous_choices,
                previous_lineage_keys,
                previous_conflicted_ids,
                participants,
            )
        };
        let (updated_choices, updated_lineage_keys, updated_conflicted_ids) = {
            let coordinator = lock_coordinator(&self.control_plane);
            (
                coordinator.conflict_choices().clone(),
                coordinator.conflict_lineage_current_keys().clone(),
                coordinator.conflicted_candidate_ids().clone(),
            )
        };
        let authoritative = match persist_conflict_choice(
            conflict_key,
            candidate_id,
            participants,
            expected_preference_revision,
        )
        .await
        {
            Ok(authoritative) => authoritative,
            Err(error) => {
                let mut coordinator = lock_coordinator(&self.control_plane);
                coordinator.replace_conflict_choices(previous_choices);
                coordinator.replace_conflict_lineage_current_keys(previous_lineage_keys);
                coordinator.replace_conflicted_candidate_ids(previous_conflicted_ids);
                return Err(error);
            }
        };
        if authoritative.conflict_choices != updated_choices
            || authoritative.conflict_lineage_current_keys != updated_lineage_keys
            || authoritative.conflicted_candidate_ids != updated_conflicted_ids
        {
            log::debug!("External source conflict preferences changed in another workspace");
        }
        propagate_conflict_preferences(&authoritative);
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        self.rebuild_product_snapshot(command_snapshot).await
    }

    async fn set_tool_target_decision(
        &self,
        approval_key: &str,
        decision_key: &str,
        approved: bool,
        expected_preference_revision: u64,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        // Keep preview validation, preference persistence and the resulting
        // product rebuild in the same ordering domain as watcher refreshes.
        // Otherwise an approval for content v1 could be persisted after a
        // refresh installs v2 with the same capability-based approval key.
        #[cfg(test)]
        self.tool_decision_gate_waiting.notify_one();
        let _refresh_guard = self.refresh_gate.lock().await;
        #[cfg(test)]
        self.tool_decision_gate_acquired.notify_one();
        let snapshot = self.snapshot();
        if snapshot.preference_revision != expected_preference_revision {
            return Err(stale_operation_error(
                "External tool preferences changed; refresh before retrying",
            ));
        }
        let source_key = snapshot
            .tool_approval_requests
            .iter()
            .find(|request| {
                request.approval_key == approval_key && request.decision_key == decision_key
            })
            .map(|request| request.target_id.source.clone())
            .or_else(|| {
                snapshot
                    .tools
                    .iter()
                    .find(|tool| {
                        tool.approval_key == approval_key && tool.decision_key == decision_key
                    })
                    .map(|tool| tool.definition.id.target.source.clone())
            })
            .ok_or_else(|| {
                missing_candidate_error("External tool decision is stale or no longer available")
            })?;
        if approved {
            ensure_source_capability_active(&snapshot, &source_key, EXTERNAL_CAPABILITY_TOOL)?;
        }
        validate_conflict_preference(approval_key, decision_key)?;
        let preferences = persist_tool_target_decision(
            approval_key,
            decision_key,
            approved,
            expected_preference_revision,
        )
        .await?;
        propagate_tool_preferences(&preferences);
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        self.rebuild_product_snapshot(command_snapshot).await
    }

    async fn set_tool_targets_enabled(
        &self,
        decisions: Vec<(String, String)>,
        enabled: bool,
        expected_catalog_generation: u64,
        expected_preference_revision: u64,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let _refresh_guard = self.refresh_gate.lock().await;
        let snapshot = self.snapshot();
        if snapshot.generation != expected_catalog_generation
            || snapshot.preference_revision != expected_preference_revision
        {
            return Err(stale_operation_error(
                "External tool catalog changed; refresh before retrying",
            ));
        }
        for (approval_key, decision_key) in &decisions {
            let source = snapshot
                .tools
                .iter()
                .find(|tool| {
                    tool.approval_key == *approval_key && tool.decision_key == *decision_key
                })
                .map(|tool| &tool.definition.id.target.source)
                .ok_or_else(|| {
                    missing_candidate_error("External tool decision is no longer available")
                })?;
            if enabled {
                ensure_source_capability_active(&snapshot, source, EXTERNAL_CAPABILITY_TOOL)?;
            }
        }
        let preferences = persist_capability_bulk_decisions(
            ExternalCapabilityBulkDecisionSet::Tool(decisions),
            enabled,
            expected_preference_revision,
        )
        .await?;
        propagate_tool_preferences(&preferences);
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        self.rebuild_product_snapshot(command_snapshot).await
    }

    async fn set_subagents_enabled(
        &self,
        decisions: Vec<(String, String)>,
        enabled: bool,
        expected_subagent_generation: u64,
        expected_preference_revision: u64,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let _refresh_guard = self.refresh_gate.lock().await;
        let snapshot = self.snapshot();
        if snapshot.subagent_generation != expected_subagent_generation
            || snapshot.preference_revision != expected_preference_revision
        {
            return Err(stale_operation_error(
                "External subagent catalog changed; refresh before retrying",
            ));
        }
        for (candidate_id, decision_key) in &decisions {
            let summary = snapshot
                .subagents
                .iter()
                .find(|summary| {
                    summary.candidate_id.as_str() == candidate_id
                        && summary.decision_key == *decision_key
                })
                .ok_or_else(|| {
                    missing_candidate_error("External subagent decision is no longer available")
                })?;
            if enabled {
                ensure_source_set_capability_active(
                    &snapshot,
                    &summary.source_keys,
                    EXTERNAL_CAPABILITY_SUBAGENT,
                )?;
            }
        }
        let preferences = persist_capability_bulk_decisions(
            ExternalCapabilityBulkDecisionSet::Subagent(
                decisions
                    .into_iter()
                    .map(|(_, decision_key)| decision_key)
                    .collect(),
            ),
            enabled,
            expected_preference_revision,
        )
        .await?;
        propagate_subagent_preferences(&preferences);
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        self.rebuild_product_snapshot(command_snapshot).await
    }

    async fn set_mcp_servers_enabled(
        &self,
        decisions: Vec<(String, String)>,
        enabled: bool,
        expected_mcp_generation: u64,
        expected_preference_revision: u64,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let _refresh_guard = self.refresh_gate.lock().await;
        let snapshot = self.snapshot();
        if snapshot.mcp_generation != expected_mcp_generation
            || snapshot.preference_revision != expected_preference_revision
        {
            return Err(stale_operation_error(
                "External MCP catalog changed; refresh before retrying",
            ));
        }
        for (candidate_id, decision_key) in &decisions {
            let server = snapshot
                .mcp_servers
                .iter()
                .find(|server| {
                    server.candidate_id == *candidate_id && server.decision_key == *decision_key
                })
                .ok_or_else(|| {
                    missing_candidate_error("External MCP decision is no longer available")
                })?;
            if enabled {
                ensure_source_capability_active(
                    &snapshot,
                    &server.definition.id.source,
                    EXTERNAL_CAPABILITY_MCP,
                )?;
            }
        }
        let preferences = persist_capability_bulk_decisions(
            ExternalCapabilityBulkDecisionSet::Mcp(
                decisions
                    .into_iter()
                    .map(|(_, decision_key)| decision_key)
                    .collect(),
            ),
            enabled,
            expected_preference_revision,
        )
        .await?;
        propagate_mcp_preferences(&preferences);
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        self.rebuild_product_snapshot(command_snapshot).await
    }

    async fn set_tool_conflict_choice(
        &self,
        conflict_key: &str,
        candidate_id: &str,
        expected_preference_revision: u64,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let _refresh_guard = self.refresh_gate.lock().await;
        let snapshot = self.snapshot();
        if snapshot.preference_revision != expected_preference_revision {
            return Err(stale_operation_error(
                "External tool preferences changed; refresh before retrying",
            ));
        }
        let candidate = snapshot
            .tool_conflicts
            .iter()
            .find(|conflict| conflict.conflict_key == conflict_key)
            .and_then(|conflict| {
                conflict
                    .candidates
                    .iter()
                    .find(|candidate| candidate.candidate_id == candidate_id)
            })
            .ok_or_else(|| {
                missing_candidate_error(
                    "External tool conflict choice is stale or no longer available",
                )
            })?;
        if matches!(candidate.kind, ExternalToolConflictCandidateKind::External) {
            match candidate.source.as_ref() {
                Some(source_key) => {
                    ensure_source_capability_active(
                        &snapshot,
                        source_key,
                        EXTERNAL_CAPABILITY_TOOL,
                    )?;
                }
                None if candidate.provider_id == "opencode-plugin" => {}
                None => {
                    return Err(missing_candidate_error(
                        "External tool conflict source is missing",
                    ));
                }
            }
        }
        validate_conflict_preference(conflict_key, candidate_id)?;
        let preferences =
            persist_tool_conflict_choice(conflict_key, candidate_id, expected_preference_revision)
                .await?;
        propagate_tool_preferences(&preferences);
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        self.rebuild_product_snapshot(command_snapshot).await
    }

    async fn set_mcp_server_decision(
        &self,
        candidate_id: &str,
        decision_key: &str,
        approved: bool,
        expected_mcp_generation: u64,
        expected_preference_revision: u64,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let _refresh_guard = self.refresh_gate.lock().await;
        let snapshot = self.snapshot();
        if snapshot.mcp_generation != expected_mcp_generation
            || snapshot.preference_revision != expected_preference_revision
        {
            return Err(stale_operation_error(
                "External MCP catalog changed; refresh before retrying",
            ));
        }
        let entry = snapshot
            .mcp_servers
            .iter()
            .find(|entry| entry.candidate_id == candidate_id && entry.decision_key == decision_key)
            .ok_or_else(|| {
                missing_candidate_error("External MCP candidate is no longer available")
            })?;
        if approved {
            ensure_source_capability_active(
                &snapshot,
                &entry.definition.id.source,
                EXTERNAL_CAPABILITY_MCP,
            )?;
        }
        if !external_mcp_decision_allowed(&entry.activation_state, approved) {
            return Err(unavailable_operation_error(
                "External MCP candidate cannot be changed in its current state",
            ));
        }
        validate_mcp_decision_value(candidate_id, "candidate id")?;
        validate_mcp_decision_value(decision_key, "decision key")?;
        let preferences =
            persist_mcp_server_decision(decision_key, approved, expected_preference_revision)
                .await?;
        propagate_mcp_preferences(&preferences);
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        self.rebuild_product_snapshot(command_snapshot).await
    }

    async fn choose_mcp_conflict(
        &self,
        conflict_key: &str,
        candidate_id: &str,
        approve_external: bool,
        expected_mcp_generation: u64,
        expected_preference_revision: u64,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let _refresh_guard = self.refresh_gate.lock().await;
        let snapshot = self.snapshot();
        if snapshot.mcp_generation != expected_mcp_generation
            || snapshot.preference_revision != expected_preference_revision
        {
            return Err(stale_operation_error(
                "External MCP catalog changed; refresh before retrying",
            ));
        }
        let conflict = snapshot
            .mcp_conflicts
            .iter()
            .find(|conflict| conflict.conflict_key == conflict_key)
            .ok_or_else(|| {
                conflict_operation_error("External MCP conflict is stale or no longer available")
            })?;
        let candidate = conflict
            .candidates
            .iter()
            .find(|candidate| candidate.candidate_id == candidate_id && candidate.available)
            .ok_or_else(|| {
                missing_candidate_error("External MCP conflict candidate is unavailable")
            })?;
        let external_decision = if candidate.external {
            let source_key = candidate.source.as_ref().ok_or_else(|| {
                missing_candidate_error("External MCP conflict source is missing")
            })?;
            ensure_source_capability_active(&snapshot, source_key, EXTERNAL_CAPABILITY_MCP)?;
            if !approve_external {
                return Err(policy_limited_error(
                    "Selecting an external MCP server requires approval of its current behavior",
                ));
            }
            let entry = snapshot
                .mcp_servers
                .iter()
                .find(|entry| entry.candidate_id == candidate_id)
                .ok_or_else(|| {
                    missing_candidate_error("External MCP candidate is no longer available")
                })?;
            Some(entry.decision_key.as_str())
        } else {
            None
        };
        validate_mcp_decision_value(conflict_key, "conflict key")?;
        validate_mcp_decision_value(candidate_id, "candidate id")?;
        let preferences = persist_mcp_conflict_choice(
            conflict_key,
            candidate_id,
            external_decision,
            expected_preference_revision,
        )
        .await?;
        propagate_mcp_preferences(&preferences);
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        self.rebuild_product_snapshot(command_snapshot).await
    }

    async fn set_subagent_activation(
        &self,
        candidate_id: &str,
        approved: bool,
        expected_subagent_generation: u64,
        expected_preference_revision: u64,
        decision_key: &str,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let _refresh_guard = self.refresh_gate.lock().await;
        let snapshot = self.snapshot();
        if snapshot.subagent_generation != expected_subagent_generation
            || snapshot.preference_revision != expected_preference_revision
        {
            return Err(stale_operation_error(
                "External subagent catalog changed; refresh before retrying",
            ));
        }
        let summary = snapshot
            .subagents
            .iter()
            .find(|summary| {
                summary.candidate_id == candidate_id && summary.decision_key == decision_key
            })
            .ok_or_else(|| {
                missing_candidate_error("External subagent candidate is no longer available")
            })?;
        if approved {
            ensure_source_set_capability_active(
                &snapshot,
                &summary.source_keys,
                EXTERNAL_CAPABILITY_SUBAGENT,
            )?;
        }
        if matches!(
            summary.activation_state,
            ExternalSubagentActivationState::Blocked
                | ExternalSubagentActivationState::Unavailable
                | ExternalSubagentActivationState::Conflict
        ) {
            return Err(unavailable_operation_error(
                "External subagent cannot be activated in its current state",
            ));
        }
        validate_subagent_decision_value(candidate_id, "candidate id")?;
        validate_subagent_decision_value(decision_key, "decision key")?;
        let preferences =
            persist_subagent_activation(decision_key, approved, expected_preference_revision)
                .await?;
        propagate_subagent_preferences(&preferences);
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        self.rebuild_product_snapshot(command_snapshot).await
    }

    async fn set_subagent_model_binding(
        &self,
        binding_key: &str,
        target: Option<ExternalSubagentModelBindingTarget>,
        expected_subagent_generation: u64,
        expected_preference_revision: u64,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let _refresh_guard = self.refresh_gate.lock().await;
        let snapshot = self.snapshot();
        validate_subagent_model_binding_mutation(
            &snapshot,
            binding_key,
            target.as_ref(),
            expected_subagent_generation,
            expected_preference_revision,
        )?;
        let preferences =
            persist_subagent_model_binding(binding_key, target, expected_preference_revision)
                .await?;
        propagate_subagent_preferences(&preferences);
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        self.rebuild_product_snapshot(command_snapshot).await
    }

    async fn choose_subagent_conflict(
        &self,
        conflict_key: &str,
        candidate_id: &str,
        approve_external: bool,
        expected_subagent_generation: u64,
        expected_preference_revision: u64,
    ) -> Result<ExternalSourceCatalogSnapshot, String> {
        let _refresh_guard = self.refresh_gate.lock().await;
        let snapshot = self.snapshot();
        if snapshot.subagent_generation != expected_subagent_generation
            || snapshot.preference_revision != expected_preference_revision
        {
            return Err(stale_operation_error(
                "External subagent catalog changed; refresh before retrying",
            ));
        }
        let conflict = snapshot
            .subagent_conflicts
            .iter()
            .find(|conflict| conflict.conflict_key == conflict_key)
            .ok_or_else(|| {
                conflict_operation_error(
                    "External subagent conflict is stale or no longer available",
                )
            })?;
        let external = if candidate_id == DISABLED_SUBAGENT_CONFLICT_CHOICE {
            false
        } else {
            conflict
                .candidates
                .iter()
                .find(|candidate| candidate.candidate_id == candidate_id)
                .map(|candidate| candidate.external)
                .ok_or_else(|| {
                    missing_candidate_error("Conflict candidate is no longer available")
                })?
        };
        let approval_key = if external {
            let summary = snapshot
                .subagents
                .iter()
                .find(|summary| summary.candidate_id == candidate_id)
                .ok_or_else(|| {
                    missing_candidate_error("External subagent candidate is no longer available")
                })?;
            ensure_source_set_capability_active(
                &snapshot,
                &summary.source_keys,
                EXTERNAL_CAPABILITY_SUBAGENT,
            )?;
            if !approve_external {
                return Err(policy_limited_error(
                    "Selecting an external subagent requires approval of its current capability envelope",
                ));
            }
            Some(summary.decision_key.clone())
        } else {
            None
        };
        validate_subagent_decision_value(conflict_key, "conflict key")?;
        validate_subagent_decision_value(candidate_id, "candidate id")?;
        let preferences = persist_subagent_conflict_choice(
            conflict_key,
            candidate_id,
            approval_key.as_deref(),
            expected_preference_revision,
        )
        .await?;
        propagate_subagent_preferences(&preferences);
        let command_snapshot = lock_coordinator(&self.control_plane).snapshot();
        self.rebuild_product_snapshot(command_snapshot).await
    }

    async fn expand_command(
        self: &Arc<Self>,
        name: &str,
        arguments: &str,
        native_commands: &[NativePromptCommandDescriptor],
        expected_candidate_id: Option<&str>,
        expected_content_version: Option<&str>,
        expected_native_conflict_key: Option<&str>,
        expected_preference_revision: Option<u64>,
        shell_review_decision: Option<&PromptCommandShellReviewDecision>,
    ) -> Result<PromptCommandInvocationOutcome, String> {
        // Explicit invocation refreshes first, so a stable deletion cannot be
        // bypassed by an old menu projection.
        let snapshot = self.refresh_preserving_worker_recovery().await?;
        let preferences = read_external_sources_config().await?;
        let native_conflicts = project_native_prompt_command_conflicts(
            &snapshot,
            native_commands,
            &preferences.conflict_choices,
            &preferences.conflicted_candidate_ids,
            preferences.preference_revision,
        )?;
        let current_native_conflicts = native_conflicts
            .conflicts
            .iter()
            .filter(|conflict| conflict.command_name.eq_ignore_ascii_case(name))
            .collect::<Vec<_>>();
        validate_native_prompt_command_expansion_guard(
            &current_native_conflicts,
            &preferences,
            expected_candidate_id,
            expected_native_conflict_key,
            expected_preference_revision,
        )?;
        let selected_command = snapshot
            .commands
            .iter()
            .find(|entry| entry.definition.name.eq_ignore_ascii_case(name))
            .map(|entry| entry.definition.clone())
            .ok_or_else(|| {
                missing_candidate_error(format!("External prompt command '{name}' was not found"))
            })?;
        let source_key = selected_command.id.source.clone();
        let execution_target = selected_command.execution_target.clone();
        ensure_source_capability_active(&snapshot, &source_key, EXTERNAL_CAPABILITY_COMMAND)?;
        let source_display_name = snapshot
            .sources
            .iter()
            .find(|source| source.record.key == source_key)
            .map(|source| source.record.display_name.clone())
            .unwrap_or_else(|| "External prompt command".to_string());
        let coordinator = Arc::clone(&self.control_plane);
        let name = name.to_string();
        let arguments = arguments.to_string();
        let expected_candidate_id = expected_candidate_id.map(str::to_string);
        let expected_content_version = expected_content_version.map(str::to_string);
        let expansion = tokio::task::spawn_blocking(move || {
            lock_coordinator(&coordinator)
                .expand_command_guarded(
                    &name,
                    &arguments,
                    expected_candidate_id.as_deref(),
                    expected_content_version.as_deref(),
                )
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| format!("external command expansion task failed: {error}"))??;
        let Some(shell_expansion) = expansion.shell.as_ref() else {
            let expanded =
                finalize_prompt_command_expansion(self.workspace_root.as_deref(), expansion)
                    .await?;
            return Ok(PromptCommandInvocationOutcome::Ready {
                content: expanded.content,
                execution_target,
            });
        };
        if self.safe_mode_enabled() {
            return Err(policy_limited_error(
                "Shell-backed external prompt commands are disabled while External Sources safe mode is active",
            ));
        }
        let resolved_shell = resolve_prompt_command_shell(&shell_expansion.preference)?;
        let plan = prepare_prompt_command_shell_plan(
            expansion,
            &source_display_name,
            self.execution_domain_id.as_str(),
            &selected_command.id.stable_key(),
            &selected_command.content_version,
            preferences.preference_revision,
            resolved_shell,
        )?;
        let approved = preferences
            .approved_prompt_command_shell_plans
            .contains(&plan.review.plan_fingerprint);
        let run = if approved {
            true
        } else if let Some(decision) = shell_review_decision {
            if decision.plan_fingerprint != plan.review.plan_fingerprint
                || decision.expected_preference_revision != preferences.preference_revision
            {
                return Ok(PromptCommandInvocationOutcome::ReviewRequired {
                    review: plan.review,
                });
            }
            match decision.mode {
                PromptCommandShellReviewMode::RunOnce => true,
                PromptCommandShellReviewMode::Remember => {
                    if !plan.review.can_remember {
                        return Err(
                            "argument-dependent prompt command shell directives cannot be remembered"
                                .to_string(),
                        );
                    }
                    let updated = persist_prompt_command_shell_plan_approval(
                        &plan.review.plan_fingerprint,
                        decision.expected_preference_revision,
                    )
                    .await?;
                    propagate_prompt_command_preferences(&updated);
                    true
                }
            }
        } else {
            false
        };
        if !run {
            return Ok(PromptCommandInvocationOutcome::ReviewRequired {
                review: plan.review,
            });
        }
        let expansion = execute_prompt_command_shell_plan(plan).await?;
        let expanded =
            finalize_prompt_command_expansion(self.workspace_root.as_deref(), expansion).await?;
        Ok(PromptCommandInvocationOutcome::Ready {
            content: expanded.content,
            execution_target,
        })
    }

    async fn start_watching(self: &Arc<Self>) {
        let Ok(policy) = self.watch_policy().await else {
            return;
        };
        let watch_roots = self.watch_roots(&policy);
        if watch_roots.is_empty() && self.profile != ExternalSourceServiceProfile::DiscoveryCatalog
        {
            return;
        }
        self.ensure_watch_roots(&policy).await;
        let mut receiver = self.watcher.subscribe();
        let weak: Weak<Self> = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let events = match receiver.recv().await {
                    Ok(events) => events,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Some(service) = weak.upgrade() {
                            let _ = service.refresh_after_source_change().await;
                            continue;
                        }
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let Some(service) = weak.upgrade() else {
                    break;
                };
                let Ok(policy) = service.watch_policy().await else {
                    continue;
                };
                let watch_roots = service.watch_roots(&policy);
                let relevant = events.iter().any(|event| {
                    let path = Path::new(&event.path);
                    watch_roots.iter().any(|root| path.starts_with(&root.path))
                });
                if !relevant {
                    continue;
                }
                if let Err(error) = service.refresh_after_source_change().await {
                    log::warn!(
                        "External source background refresh failed scope={} error_category={}",
                        external_log_scope(service.workspace_root.as_deref()),
                        external_log_error_category(&error),
                    );
                }
            }
        });
    }

    fn start_model_config_watching(self: &Arc<Self>) {
        let Some(mut receiver) = subscribe_config_updates() else {
            return;
        };
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let should_refresh = match receiver.recv().await {
                    Ok(event) => config_update_refreshes_external_model_bindings(&event),
                    Err(broadcast::error::RecvError::Lagged(_)) => true,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                if !should_refresh {
                    continue;
                }
                let Some(service) = weak.upgrade() else {
                    break;
                };
                let command_snapshot = lock_coordinator(&service.control_plane).snapshot();
                match service.rebuild_product_snapshot(command_snapshot).await {
                    Ok(snapshot) => {
                        let _ = service.updates.send(snapshot);
                    }
                    Err(error) => log::warn!(
                        "External source model-binding refresh failed scope={} error_category={}",
                        external_log_scope(service.workspace_root.as_deref()),
                        external_log_error_category(&error),
                    ),
                }
            }
        });
    }

    async fn ensure_watch_roots(&self, policy: &ExternalIntegrationPolicySnapshot) {
        let discovery_policy;
        let policy = if self.profile == ExternalSourceServiceProfile::DiscoveryCatalog {
            discovery_policy = match self.watch_policy().await {
                Ok(policy) => policy,
                Err(_) => return,
            };
            &discovery_policy
        } else {
            policy
        };
        let watch_roots = self.watch_roots(policy);
        let watcher = Arc::clone(&self.watcher);
        let mut states = self.watch_states.lock().await;
        let desired = watch_roots
            .iter()
            .map(|root| (root.path.clone(), root.recursive))
            .collect::<BTreeSet<_>>();
        let obsolete = states
            .keys()
            .filter(|key| !desired.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        for key in obsolete {
            if states.get(&key).copied().unwrap_or(false) {
                let path = key.0.to_string_lossy().to_string();
                if let Err(error) = watcher.unwatch_path(&path).await {
                    log::warn!(
                        "Failed to stop watching external source root scope={} recursive={} error_category={}",
                        external_log_scope(self.workspace_root.as_deref()),
                        key.1,
                        external_log_error_category(&error.to_string()),
                    );
                }
            }
            states.remove(&key);
        }
        for root in watch_roots {
            let key = (root.path.clone(), root.recursive);
            let exists = root.path.exists();
            let was_available = states.get(&key).copied().unwrap_or(false);
            if !exists {
                states.insert(key, false);
                continue;
            }
            if was_available {
                continue;
            }
            let mut config = FileWatcherConfig::default();
            config.watch_recursively = root.recursive;
            config.ignore_hidden_files = false;
            config.debounce_interval_ms = 350;
            let path = root.path.to_string_lossy().to_string();
            match watcher.watch_path(&path, Some(config)).await {
                Ok(()) => {
                    states.insert(key, true);
                }
                Err(error) => {
                    states.insert(key, false);
                    log::warn!(
                        "Failed to watch external source root scope={} recursive={} error_category={}",
                        external_log_scope(self.workspace_root.as_deref()),
                        root.recursive,
                        external_log_error_category(&error.to_string()),
                    );
                }
            }
        }
    }

    fn watch_roots(
        &self,
        policy: &ExternalIntegrationPolicySnapshot,
    ) -> Vec<openbitfun_product_domains::external_sources::ExternalWatchRoot> {
        let mut roots = BTreeMap::new();
        let mut provider_roots = Vec::new();
        let command_ecosystems =
            ecosystems_with_discoverable_capability(policy, EXTERNAL_CAPABILITY_COMMAND);
        provider_roots.extend(
            lock_coordinator(&self.control_plane).watch_roots_for_ecosystems(&command_ecosystems),
        );
        let tool_ecosystems =
            ecosystems_with_discoverable_capability(policy, EXTERNAL_CAPABILITY_TOOL);
        provider_roots.extend(
            lock_tool_coordinator(&self.control_plane).watch_roots_for_ecosystems(&tool_ecosystems),
        );
        let subagent_ecosystems =
            ecosystems_with_discoverable_capability(policy, EXTERNAL_CAPABILITY_SUBAGENT);
        provider_roots.extend(
            lock_subagent_coordinator(&self.control_plane)
                .watch_roots_for_ecosystems(&subagent_ecosystems),
        );
        let mcp_ecosystems =
            ecosystems_with_discoverable_capability(policy, EXTERNAL_CAPABILITY_MCP);
        provider_roots.extend(
            lock_mcp_coordinator(&self.control_plane).watch_roots_for_ecosystems(&mcp_ecosystems),
        );
        let workspace_reference_ecosystems =
            ecosystems_with_discoverable_capability(policy, EXTERNAL_CAPABILITY_REFERENCE);
        provider_roots.extend(
            lock_workspace_reference_coordinator(&self.control_plane)
                .watch_roots_for_ecosystems(&workspace_reference_ecosystems),
        );
        for root in provider_roots {
            roots
                .entry(root.path)
                .and_modify(|recursive| *recursive |= root.recursive)
                .or_insert(root.recursive);
        }
        if let Ok(store) = ExternalSourcePreferenceStore::global() {
            if let Some(parent) = store.path.parent() {
                roots.entry(parent.to_path_buf()).or_insert(false);
            }
        }
        roots
            .into_iter()
            .map(|(path, recursive)| {
                openbitfun_product_domains::external_sources::ExternalWatchRoot { path, recursive }
            })
            .collect()
    }
}

fn lock_coordinator(
    control_plane: &ExternalSourceControlPlane,
) -> MutexGuard<'_, openbitfun_external_sources::ExternalSourceCoordinator> {
    control_plane.lock_commands()
}

fn lock_tool_coordinator(
    control_plane: &ExternalSourceControlPlane,
) -> MutexGuard<'_, openbitfun_external_sources::ExternalToolCoordinator> {
    control_plane.lock_tools()
}

fn lock_subagent_coordinator(
    control_plane: &ExternalSourceControlPlane,
) -> MutexGuard<'_, openbitfun_external_sources::ExternalSubagentCoordinator> {
    control_plane.lock_subagents()
}

fn lock_mcp_coordinator(
    control_plane: &ExternalSourceControlPlane,
) -> MutexGuard<'_, openbitfun_external_sources::ExternalMcpCoordinator> {
    control_plane.lock_mcp()
}

fn lock_workspace_reference_coordinator(
    control_plane: &ExternalSourceControlPlane,
) -> MutexGuard<'_, openbitfun_external_sources::ExternalWorkspaceReferenceCoordinator> {
    control_plane.lock_workspace_references()
}

fn lock_snapshot(
    snapshot: &StdMutex<ExternalSourceCatalogSnapshot>,
) -> MutexGuard<'_, ExternalSourceCatalogSnapshot> {
    match snapshot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

static WORKSPACE_SERVICES: OnceLock<DashMap<Option<String>, Weak<WorkspaceExternalSourceService>>> =
    OnceLock::new();
static READ_ONLY_WORKSPACE_SERVICES: OnceLock<
    DashMap<Option<String>, Weak<WorkspaceExternalSourceService>>,
> = OnceLock::new();
static SAFE_MODE_WORKSPACES: OnceLock<DashMap<String, ()>> = OnceLock::new();

fn safe_mode_workspaces() -> &'static DashMap<String, ()> {
    SAFE_MODE_WORKSPACES.get_or_init(DashMap::new)
}
static TOOL_REGISTRY_CHANGE_EPOCH: AtomicU64 = AtomicU64::new(0);
static TOOL_REGISTRY_REBUILD_SCHEDULED: AtomicBool = AtomicBool::new(false);

fn workspace_services() -> &'static DashMap<Option<String>, Weak<WorkspaceExternalSourceService>> {
    WORKSPACE_SERVICES.get_or_init(DashMap::new)
}

fn read_only_workspace_services(
) -> &'static DashMap<Option<String>, Weak<WorkspaceExternalSourceService>> {
    READ_ONLY_WORKSPACE_SERVICES.get_or_init(DashMap::new)
}

fn discovery_workspace_services(
) -> &'static DashMap<Option<String>, Weak<WorkspaceExternalSourceService>> {
    static SERVICES: OnceLock<DashMap<Option<String>, Weak<WorkspaceExternalSourceService>>> =
        OnceLock::new();
    SERVICES.get_or_init(DashMap::new)
}

fn workspace_services_for_profile(
    profile: ExternalSourceServiceProfile,
) -> &'static DashMap<Option<String>, Weak<WorkspaceExternalSourceService>> {
    match profile {
        ExternalSourceServiceProfile::LocalExecution => workspace_services(),
        ExternalSourceServiceProfile::ReadOnlyProjection => read_only_workspace_services(),
        ExternalSourceServiceProfile::DiscoveryCatalog => discovery_workspace_services(),
    }
}

fn workspace_service_gate() -> &'static tokio::sync::Mutex<()> {
    static GATE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn relative_display_path(location: &str, root: &Path) -> Option<String> {
    let normalized_location = location.replace('\\', "/");
    let normalized_root = root.to_string_lossy().replace('\\', "/");
    let root = normalized_root.trim_end_matches('/');
    let prefix = normalized_location.get(..root.len())?;
    let relative = normalized_location.get(root.len()..)?;
    (prefix.eq_ignore_ascii_case(root) && relative.starts_with('/'))
        .then(|| relative.trim_start_matches('/').to_string())
        .or_else(|| (prefix.eq_ignore_ascii_case(root) && relative.is_empty()).then(String::new))
}

fn resolve_external_source_location<'a>(
    stable_key: &str,
    sources: impl IntoIterator<Item = &'a ExternalSourceCatalogEntry>,
) -> Result<PathBuf, String> {
    let location = sources
        .into_iter()
        .find(|source| source.stable_key == stable_key)
        .map(|source| source.record.location.trim())
        .filter(|location| !location.is_empty())
        .ok_or_else(|| {
            missing_candidate_error(format!(
                "External source '{stable_key}' is no longer available"
            ))
        })?;
    let path = PathBuf::from(location);
    if !path.is_absolute() {
        return Err(invalid_operation_error(
            "External source location is not an absolute host path",
        ));
    }
    Ok(path)
}

pub(super) fn safe_external_source_location(
    scope: ExternalSourceScope,
    location: &str,
    workspace_root: Option<&Path>,
) -> String {
    let normalized = location.replace('\\', "/");
    let components = normalized
        .split('/')
        .filter(|component| {
            !component.is_empty()
                && *component != "."
                && *component != ".."
                && !component.ends_with(':')
        })
        .collect::<Vec<_>>();
    let generic_tail = || {
        components
            .iter()
            .position(|component| *component == ".config")
            .map(|index| components[index..].join("/"))
            .unwrap_or_else(|| components[components.len().saturating_sub(3)..].join("/"))
    };

    match scope {
        ExternalSourceScope::Project | ExternalSourceScope::WorkspaceLocal => {
            let relative = workspace_root
                .and_then(|root| relative_display_path(&normalized, root))
                .unwrap_or_else(generic_tail);
            format!("<workspace>/{}", relative.trim_start_matches('/'))
        }
        ExternalSourceScope::UserGlobal => {
            let relative = dirs::home_dir()
                .as_deref()
                .and_then(|home| relative_display_path(&normalized, home))
                .unwrap_or_else(generic_tail);
            format!("~/{}", relative.trim_start_matches('/'))
        }
        ExternalSourceScope::RemoteUser | ExternalSourceScope::RemoteProject => {
            format!("<remote>/{}", generic_tail().trim_start_matches('/'))
        }
        _ => format!(
            "<external-source>/{}",
            generic_tail().trim_start_matches('/')
        ),
    }
}

fn assign_external_source_presentation_groups(snapshot: &mut ExternalSourceCatalogSnapshot) {
    let mut groups = BTreeMap::<(String, String, String), Vec<usize>>::new();
    for (index, source) in snapshot.sources.iter().enumerate() {
        let normalized_location = source
            .record
            .location
            .trim()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_string();
        let location_key = if normalized_location.is_empty() {
            format!("<source:{}>", source.stable_key)
        } else {
            normalized_location
        };
        groups
            .entry((
                source.record.ecosystem_id.as_str().to_string(),
                source.record.execution_domain_id.as_str().to_string(),
                location_key,
            ))
            .or_default()
            .push(index);
    }

    for indices in groups.into_values() {
        let mut stable_keys = indices
            .iter()
            .map(|index| snapshot.sources[*index].stable_key.as_str())
            .collect::<Vec<_>>();
        stable_keys.sort_unstable();
        let group_id = format!(
            "external-source:{}",
            serde_json::to_string(&stable_keys).unwrap_or_default()
        );
        for index in indices {
            snapshot.sources[index].presentation_group_id = Some(group_id.clone());
        }
    }
}

fn sanitize_external_snapshot_locations(
    snapshot: &mut ExternalSourceCatalogSnapshot,
    workspace_root: Option<&Path>,
) {
    let source_scopes = snapshot
        .sources
        .iter()
        .map(|source| (source.record.key.clone(), source.record.scope))
        .collect::<BTreeMap<_, _>>();
    let mut replacements = Vec::new();
    let mut remember_location = |scope: ExternalSourceScope, location: &str| {
        if location.is_empty() {
            return;
        }
        let safe = safe_external_source_location(scope, location, workspace_root);
        let safe_prefix = format!("{}/", safe.trim_end_matches('/'));
        for raw in [
            location.to_string(),
            location.replace('\\', "/"),
            location.replace('/', "\\"),
        ] {
            for (raw, safe) in [
                (format!("{raw}/"), safe_prefix.clone()),
                (format!("{raw}\\"), safe_prefix.clone()),
                (raw, safe.clone()),
            ] {
                if raw != safe && !replacements.iter().any(|(known, _)| known == &raw) {
                    replacements.push((raw, safe));
                }
            }
        }
    };
    for source in &snapshot.sources {
        remember_location(source.record.scope, &source.record.location);
    }
    for conflict in &snapshot.command_conflicts {
        for candidate in &conflict.candidates {
            remember_location(candidate.source_scope, &candidate.source_location);
        }
    }
    for request in &snapshot.tool_approval_requests {
        remember_location(request.source_scope, &request.source_location);
        remember_location(request.source_scope, &request.working_directory);
    }
    for tool in &snapshot.tools {
        let scope = source_scopes
            .get(&tool.definition.id.target.source)
            .copied()
            .unwrap_or(ExternalSourceScope::WorkspaceLocal);
        remember_location(scope, &tool.definition.module_path);
        remember_location(scope, &tool.definition.working_directory);
    }
    for conflict in &snapshot.tool_conflicts {
        for candidate in &conflict.candidates {
            let Some(location) = candidate.source_location.as_deref() else {
                continue;
            };
            let scope = candidate
                .source
                .as_ref()
                .and_then(|source| source_scopes.get(source))
                .copied()
                .unwrap_or(ExternalSourceScope::WorkspaceLocal);
            remember_location(scope, location);
        }
    }
    for server in &snapshot.mcp_servers {
        let Some(directory) = server.definition.working_directory.as_deref() else {
            continue;
        };
        let scope = source_scopes
            .get(&server.definition.id.source)
            .copied()
            .unwrap_or(ExternalSourceScope::WorkspaceLocal);
        remember_location(scope, directory);
    }
    for request in &snapshot.mcp_approval_requests {
        let Some(directory) = request.definition.working_directory.as_deref() else {
            continue;
        };
        let scope = source_scopes
            .get(&request.definition.id.source)
            .copied()
            .unwrap_or(ExternalSourceScope::WorkspaceLocal);
        remember_location(scope, directory);
    }
    replacements.sort_by(|left, right| right.0.len().cmp(&left.0.len()));
    let sanitize_message = |message: &mut String| {
        for (raw, safe) in &replacements {
            if message.contains(raw) {
                *message = message.replace(raw, safe);
            }
        }
    };
    for diagnostic in &mut snapshot.diagnostics {
        sanitize_message(&mut diagnostic.message);
    }
    for source in &mut snapshot.sources {
        for diagnostic in &mut source.record.diagnostics {
            sanitize_message(&mut diagnostic.message);
        }
    }

    for source in &mut snapshot.sources {
        source.record.location = safe_external_source_location(
            source.record.scope,
            &source.record.location,
            workspace_root,
        );
    }
    for conflict in &mut snapshot.command_conflicts {
        for candidate in &mut conflict.candidates {
            candidate.source_location = safe_external_source_location(
                candidate.source_scope,
                &candidate.source_location,
                workspace_root,
            );
        }
    }
    for request in &mut snapshot.tool_approval_requests {
        request.source_location = safe_external_source_location(
            request.source_scope,
            &request.source_location,
            workspace_root,
        );
        request.working_directory = safe_external_source_location(
            request.source_scope,
            &request.working_directory,
            workspace_root,
        );
    }
    for tool in &mut snapshot.tools {
        let scope = source_scopes
            .get(&tool.definition.id.target.source)
            .copied()
            .unwrap_or(ExternalSourceScope::WorkspaceLocal);
        tool.definition.module_path =
            safe_external_source_location(scope, &tool.definition.module_path, workspace_root);
        tool.definition.working_directory = safe_external_source_location(
            scope,
            &tool.definition.working_directory,
            workspace_root,
        );
    }
    for conflict in &mut snapshot.tool_conflicts {
        for candidate in &mut conflict.candidates {
            let Some(location) = candidate.source_location.as_mut() else {
                continue;
            };
            let scope = candidate
                .source
                .as_ref()
                .and_then(|source| source_scopes.get(source))
                .copied()
                .unwrap_or(ExternalSourceScope::WorkspaceLocal);
            *location = safe_external_source_location(scope, location, workspace_root);
        }
    }
    for server in &mut snapshot.mcp_servers {
        let Some(directory) = server.definition.working_directory.as_mut() else {
            continue;
        };
        let scope = source_scopes
            .get(&server.definition.id.source)
            .copied()
            .unwrap_or(ExternalSourceScope::WorkspaceLocal);
        *directory = safe_external_source_location(scope, directory, workspace_root);
    }
    for request in &mut snapshot.mcp_approval_requests {
        let Some(directory) = request.definition.working_directory.as_mut() else {
            continue;
        };
        let scope = source_scopes
            .get(&request.definition.id.source)
            .copied()
            .unwrap_or(ExternalSourceScope::WorkspaceLocal);
        *directory = safe_external_source_location(scope, directory, workspace_root);
    }
}

fn native_mcp_behavior_version(
    revision_key: &ExternalMcpRevisionKey,
    config: &crate::service::mcp::MCPServerConfig,
) -> Result<String, String> {
    let value = serde_json::to_value(config)
        .map_err(|error| format!("Could not fingerprint OpenBitFun MCP configuration: {error}"))?;
    let mut encoded = Vec::new();
    write_canonical_json(&value, &mut encoded)
        .map_err(|error| format!("Could not fingerprint OpenBitFun MCP configuration: {error}"))?;
    Ok(revision_key.opaque_revision("openbitfun.mcp.behavior.v1", [encoded.as_slice()]))
}

fn write_canonical_json(value: &serde_json::Value, output: &mut Vec<u8>) -> serde_json::Result<()> {
    match value {
        serde_json::Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        serde_json::Value::Object(values) => {
            output.push(b'{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(left, _)| *left);
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)?;
                output.push(b':');
                write_canonical_json(value, output)?;
            }
            output.push(b'}');
        }
        value => serde_json::to_writer(output, value)?,
    }
    Ok(())
}

async fn load_native_mcp_candidates(
    revision_key: &ExternalMcpRevisionKey,
) -> Result<Vec<NativeMcpCandidate>, String> {
    let service = crate::service::mcp::get_global_mcp_service()
        .ok_or_else(|| "MCP service is not initialized".to_string())?;
    let configs = service
        .config_service()
        .load_all_configs()
        .await
        .map_err(|error| format!("Could not read OpenBitFun MCP configuration: {error}"))?;
    let mut candidates = Vec::with_capacity(configs.len());
    for config in configs {
        let behavior_version = native_mcp_behavior_version(revision_key, &config)?;
        let candidate_id = native_mcp_candidate_id(&config.id);
        candidates.push(NativeMcpCandidate {
            candidate_id,
            server_id: config.id,
            display_name: format!("OpenBitFun: {}", config.name),
            name: config.name,
            behavior_version,
            enabled: config.enabled,
        });
    }
    candidates.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.server_id.cmp(&right.server_id))
    });
    Ok(candidates)
}

/// Stable product identifier for a OpenBitFun-owned MCP configuration. Surfaces
/// use this only to correlate native list rows with conflict candidates; the
/// underlying configuration id remains private to the MCP owner.
pub fn native_mcp_candidate_id(server_id: &str) -> String {
    let mut id_hasher = Sha256::new();
    id_hasher.update(server_id.as_bytes());
    format!("native_mcp:{}", &hex::encode(id_hasher.finalize())[..24])
}

fn apply_external_mcp_runtime_status(
    state: &mut ExternalMcpProductState,
    candidate: &crate::external_mcp::ActiveExternalMcpCandidate,
    status: Result<ExternalMcpRuntimeStatus, String>,
) {
    match status {
        Ok(ExternalMcpRuntimeStatus::Active) => {}
        Ok(ExternalMcpRuntimeStatus::Loading) => {
            if let Some(entry) = state
                .entries
                .iter_mut()
                .find(|entry| entry.candidate_id == candidate.definition.candidate_id())
            {
                entry.activation_state = ExternalMcpActivationState::Starting;
            }
        }
        Ok(ExternalMcpRuntimeStatus::Unavailable(reason)) | Err(reason) => {
            mark_external_mcp_runtime_unavailable(state, candidate, reason);
        }
    }
}

fn mark_external_mcp_runtime_unavailable(
    state: &mut ExternalMcpProductState,
    candidate: &crate::external_mcp::ActiveExternalMcpCandidate,
    reason: String,
) {
    let reason = stable_external_mcp_runtime_reason(&reason).to_string();
    if let Some(entry) = state
        .entries
        .iter_mut()
        .find(|entry| entry.candidate_id == candidate.definition.candidate_id())
    {
        entry.activation_state = ExternalMcpActivationState::RuntimeUnavailable {
            reason: reason.clone(),
        };
    }
    state.diagnostics.push(
        ExternalSourceDiagnostic::warning(
            "external_mcp.runtime_unavailable",
            reason,
            Some(candidate.definition.id.source.clone()),
        )
        .with_asset_kind(ExternalSourceAssetKind::Mcp),
    );
}

fn stable_external_mcp_runtime_reason(reason: &str) -> &str {
    match reason {
        "external_mcp.start.authentication"
        | "external_mcp.start.timeout"
        | "external_mcp.start.command_unavailable"
        | "external_mcp.start.working_directory_unavailable"
        | "external_mcp.start.connection_failed"
        | "external_mcp.start.protocol_failed"
        | "external_mcp.start.other"
        | "external_mcp.native_configuration_unavailable"
        | "external_mcp.runtime.host_read_only"
        | "external_mcp.runtime.configuration_changed"
        | "external_mcp.runtime.configuration_invalid"
        | "external_mcp.runtime.host_unavailable"
        | "external_mcp.runtime.install_failed"
        | "external_mcp.runtime.preparation_failed"
        | "external_mcp.runtime.retire_failed"
        | "external_mcp.runtime.server_missing"
        | "external_mcp.runtime.failed"
        | "external_mcp.runtime.stopped"
        | "external_mcp.runtime.status_unavailable" => reason,
        _ => EXTERNAL_MCP_RUNTIME_FAILED,
    }
}

fn merge_mcp_state(
    snapshot: &mut ExternalSourceCatalogSnapshot,
    coordinator_snapshot: &openbitfun_external_sources::ExternalMcpCoordinatorSnapshot,
    state: ExternalMcpProductState,
) {
    let known_sources = snapshot
        .sources
        .iter()
        .map(|source| source.stable_key.clone())
        .collect::<BTreeSet<_>>();
    snapshot.sources.extend(
        coordinator_snapshot
            .sources
            .iter()
            .filter(|source| !known_sources.contains(&source.stable_key))
            .cloned(),
    );
    snapshot.sources.sort_by(|left, right| {
        left.record
            .ecosystem_id
            .cmp(&right.record.ecosystem_id)
            .then(left.stable_key.cmp(&right.stable_key))
    });
    snapshot
        .diagnostics
        .extend(coordinator_snapshot.diagnostics.clone());
    snapshot.diagnostics.extend(state.diagnostics.clone());
    snapshot.discovery_pending |= coordinator_snapshot.discovery_pending;
    snapshot.mcp_generation = coordinator_snapshot.generation;
    snapshot.mcp_servers = state.entries;
    snapshot.mcp_approval_requests = state.approval_requests;
    snapshot.mcp_conflicts = state.conflicts;
}

async fn service_for(
    workspace_id: Option<&str>,
) -> Result<Arc<WorkspaceExternalSourceService>, String> {
    service_for_profile(workspace_id, ExternalSourceServiceProfile::LocalExecution).await
}

fn mcp_import_discovery_complete(
    snapshot: &openbitfun_external_sources::ExternalMcpCoordinatorSnapshot,
) -> bool {
    !snapshot.discovery_pending
        && !snapshot
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.starts_with("external_mcp.discovery_"))
}

pub(crate) async fn collect_external_mcp_import_candidates(
    workspace_id: Option<&str>,
) -> Result<Vec<crate::external_mcp_import::ExternalMcpImportCandidate>, String> {
    let service =
        service_for_profile(workspace_id, ExternalSourceServiceProfile::DiscoveryCatalog).await?;
    service.refresh_mcp_import_sources().await?;
    // Provider preparation performs bounded synchronous file reads. Keep it off
    // the async workers serving UI requests and connection lifecycle.
    tokio::task::spawn_blocking(move || {
        let coordinator = lock_mcp_coordinator(&service.control_plane);
        let snapshot = coordinator.snapshot();
        log::debug!(
        "External MCP import discovery: providers={:?}, servers={}, pending={}, diagnostics={:?}",
        snapshot
            .sources
            .iter()
            .map(|source| source.record.key.provider_id.as_str())
            .collect::<BTreeSet<_>>(),
        snapshot.servers.len(),
        snapshot.discovery_pending,
        snapshot
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>(),
    );
        if !mcp_import_discovery_complete(&snapshot) {
            return Err(
                "External MCP discovery is incomplete; refresh before reviewing imports"
                    .to_string(),
            );
        }
        let input_candidates = snapshot
            .servers
            .iter()
            .cloned()
            .map(|definition| {
                let ecosystem_id = coordinator
                    .ecosystem_for_provider(&definition.id.source.provider_id)
                    .ok_or_else(|| "External MCP provider ecosystem is unavailable".to_string())?;
                let preparation = match coordinator
                    .prepare_import_guarded(&definition.id, &definition.behavior_version)
                {
                    Ok(prepared) => {
                        crate::external_mcp_import::ExternalMcpImportPreparation::Prepared(prepared)
                    }
                    Err(error) => {
                        crate::external_mcp_import::ExternalMcpImportPreparation::Unavailable(
                            error.code,
                        )
                    }
                };
                Ok(crate::external_mcp_import::ExternalMcpImportCandidate {
                    definition,
                    ecosystem_id,
                    preparation,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(input_candidates)
    })
    .await
    .map_err(|error| format!("External MCP import preparation failed: {error}"))?
}

async fn read_only_service_for(
    workspace_id: Option<&str>,
) -> Result<Arc<WorkspaceExternalSourceService>, String> {
    service_for_profile(
        workspace_id,
        ExternalSourceServiceProfile::ReadOnlyProjection,
    )
    .await
}

async fn service_for_profile(
    workspace_id: Option<&str>,
    profile: ExternalSourceServiceProfile,
) -> Result<Arc<WorkspaceExternalSourceService>, String> {
    let workspace = match workspace_id {
        Some(id) => {
            let registry = crate::service::workspace::get_global_workspace_service()
                .ok_or("Workspace service is unavailable")?;
            let record = registry
                .require_workspace(id)
                .await
                .map_err(|error| error.to_string())?;
            if record.workspace_kind == crate::service::workspace::WorkspaceKind::Remote {
                return Err("External source discovery does not support remote workspaces".into());
            }
            Some(record)
        }
        None => None,
    };
    if let Some(id) = workspace_id {
        ExternalSourcePreferenceStore::global()?
            .upgrade_workspace_policy(id)
            .await?;
    }
    let workspace_root = workspace.as_ref().map(|record| record.root_path.clone());
    let workspace_id = workspace_id.map(str::to_owned);
    // Serialize cache acquisition with idle retirement. Without this lease
    // gate, a caller could upgrade the weak entry after the retirement count
    // check and have its newly acquired routes removed underneath it.
    let _service_gate = workspace_service_gate().lock().await;
    let services = workspace_services_for_profile(profile);
    if let Some(service) = services
        .get(&workspace_id)
        .and_then(|service| service.value().upgrade())
    {
        service.touch();
        sync_service_preferences(&service).await?;
        drop(_service_gate);
        if profile == ExternalSourceServiceProfile::DiscoveryCatalog {
            let enabled = service.automatic_discovery_allowed().await?;
            let was_enabled = service
                .automatic_discovery_active
                .swap(enabled, Ordering::AcqRel);
            if enabled && !was_enabled {
                service
                    .background_refresh_scheduled
                    .store(true, Ordering::Release);
                let result = service.refresh_preserving_worker_recovery().await;
                service
                    .background_refresh_scheduled
                    .store(false, Ordering::Release);
                result?;
            }
        }
        return Ok(service);
    }
    let created = WorkspaceExternalSourceService::create(
        workspace_id.clone(),
        workspace_root.clone(),
        profile,
    )
    .await?;
    let service = match services.entry(workspace_id) {
        Entry::Occupied(mut entry) => match entry.get().upgrade() {
            Some(existing) => existing,
            None => {
                entry.insert(Arc::downgrade(&created));
                created
            }
        },
        Entry::Vacant(entry) => {
            entry.insert(Arc::downgrade(&created));
            created
        }
    };
    service.touch();
    service.ensure_idle_keepalive();
    sync_service_preferences(&service).await?;
    Ok(service)
}

fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn read_external_sources_config() -> Result<ExternalSourcesConfig, String> {
    ExternalSourcePreferenceStore::global()?.read().await
}

fn acknowledged_ecosystem_key(execution_domain_id: &str, ecosystem_id: &str) -> String {
    format!("{execution_domain_id}\u{1f}{ecosystem_id}")
}

/// Ecosystems that have configuration on this host but have never been
/// announced to the user.
///
/// Both the desktop settings navigation and the TUI read this same result, so
/// neither surface derives "is there something new" on its own and they cannot
/// drift apart. An ecosystem only qualifies once discovery actually found a
/// source for it: a registered adapter with nothing to offer is not news.
pub async fn unacknowledged_external_ecosystems(
    workspace_id: Option<&str>,
) -> Result<Vec<String>, String> {
    let service = read_only_service_for(workspace_id).await?;
    let execution_domain_id = service.execution_domain_id.clone();
    let discovered = service
        .snapshot()
        .sources
        .iter()
        .map(|source| source.record.ecosystem_id.to_string())
        .collect::<BTreeSet<_>>();
    if discovered.is_empty() {
        return Ok(Vec::new());
    }
    let config = read_external_sources_config().await?;
    Ok(discovered
        .into_iter()
        .filter(|ecosystem_id| {
            !config
                .acknowledged_ecosystems
                .contains(&acknowledged_ecosystem_key(
                    execution_domain_id.as_str(),
                    ecosystem_id,
                ))
        })
        .collect())
}

/// Records that the user has seen the given ecosystems.
///
/// Awareness is not part of the preference-revision contract. The set only
/// grows, insertion is idempotent, and no policy or approval decision reads it,
/// so concurrent writers cannot lose each other's decisions here. Taking an
/// expected revision would therefore add fencing failures without protecting
/// anything, and bumping the revision would invalidate unrelated in-flight
/// mutations every time a user opens the settings page.
///
/// The execution domain is resolved from the workspace's own service so hosts
/// never pass an identity that disagrees with the one discovery recorded.
pub async fn acknowledge_external_ecosystems(
    workspace_id: Option<&str>,
    ecosystem_ids: Vec<String>,
) -> Result<(), String> {
    if ecosystem_ids.is_empty() {
        return Ok(());
    }
    let execution_domain_id = read_only_service_for(workspace_id)
        .await?
        .execution_domain_id
        .clone();
    let keys = ecosystem_ids
        .iter()
        .map(|ecosystem_id| acknowledged_ecosystem_key(execution_domain_id.as_str(), ecosystem_id))
        .collect::<Vec<_>>();
    ExternalSourcePreferenceStore::global()?
        .update(move |config| {
            for key in &keys {
                if config.acknowledged_ecosystems.contains(key) {
                    continue;
                }
                if config.acknowledged_ecosystems.len() >= MAX_ACKNOWLEDGED_ECOSYSTEMS {
                    break;
                }
                config.acknowledged_ecosystems.insert(key.clone());
            }
            true
        })
        .await
        .map(|_| ())
}

async fn persist_prompt_command_shell_plan_approval(
    fingerprint: &str,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    let fingerprint = fingerprint.to_string();
    ExternalSourcePreferenceStore::global()?
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return false;
            }
            if config
                .approved_prompt_command_shell_plans
                .contains(&fingerprint)
            {
                return true;
            }
            if config.approved_prompt_command_shell_plans.len()
                >= MAX_APPROVED_PROMPT_COMMAND_SHELL_PLANS
            {
                return false;
            }
            config
                .approved_prompt_command_shell_plans
                .insert(fingerprint);
            config.preference_revision = config.preference_revision.saturating_add(1);
            true
        })
        .await
        .and_then(|(applied, config)| {
            applied.then_some(config).ok_or_else(|| {
                stale_operation_error(
                    "Prompt command shell approvals changed or reached their storage limit; refresh before retrying",
                )
            })
        })
}

pub(crate) async fn external_tool_invocation_is_authorized(
    ecosystem_id: &str,
    approval_key: &str,
    source_key: &str,
    workspace_route: &str,
) -> Result<bool, String> {
    if external_source_safe_mode_enabled_for(LEGACY_LOCAL_EXECUTION_DOMAIN_ID, workspace_route) {
        return Ok(false);
    }
    let preferences = read_external_sources_config().await?;
    Ok(external_tool_invocation_is_authorized_by(
        &preferences,
        ecosystem_id,
        approval_key,
        source_key,
        workspace_route,
    ))
}

fn external_tool_invocation_is_authorized_by(
    preferences: &ExternalSourcesConfig,
    ecosystem_id: &str,
    approval_key: &str,
    source_preference_key: &str,
    workspace_route: &str,
) -> bool {
    let policy = preferences.integration_policy.known().map(|document| {
        external_integration_policy_snapshot(
            document,
            workspace_policy_key_from_route(workspace_route).as_deref(),
            default_external_integration_ecosystems(),
        )
    });
    policy.is_some_and(|policy| {
        policy.is_ok_and(|policy| {
            integration_capability_is_active(&policy, ecosystem_id, EXTERNAL_CAPABILITY_TOOL)
        })
    }) && preferences.approved_tool_targets.contains(approval_key)
        && !preferences
            .suppressed_source_keys
            .iter()
            .any(|suppressed| suppressed == source_preference_key)
}

pub(crate) async fn external_tool_conflict_selection_is_current(
    conflict_key: &str,
    candidate_id: Option<&str>,
) -> Result<bool, String> {
    let preferences = read_external_sources_config().await?;
    let persisted = preferences
        .tool_conflict_choices
        .get(conflict_key)
        .map(String::as_str)
        .filter(|choice| {
            *choice != UNRESOLVED_TOOL_CONFLICT_CHOICE
                && *choice != TOOL_CONFLICT_RESELECTION_REQUIRED
        });
    Ok(persisted == candidate_id)
}

async fn persist_observed_tool_conflicts(conflicts: &[ExternalToolConflict]) -> Result<(), String> {
    if conflicts.is_empty() {
        return Ok(());
    }
    let conflicts = conflicts.to_vec();
    ExternalSourcePreferenceStore::global()?
        .update(move |config| {
            let previous = config.tool_conflict_choices.clone();
            for conflict in conflicts {
                reconcile_observed_tool_conflict(
                    &mut config.tool_conflict_choices,
                    &conflict.conflict_key,
                );
            }
            if config.tool_conflict_choices != previous {
                config.preference_revision = config.preference_revision.saturating_add(1);
            }
        })
        .await
        .map(|_| ())
}

async fn persist_observed_subagent_conflicts(
    observed: &BTreeMap<String, String>,
) -> Result<(bool, ExternalSourcesConfig), String> {
    let store = ExternalSourcePreferenceStore::global()?;
    persist_observed_subagent_conflicts_with_store(&store, observed).await
}

async fn persist_observed_subagent_conflicts_with_store(
    store: &ExternalSourcePreferenceStore,
    observed: &BTreeMap<String, String>,
) -> Result<(bool, ExternalSourcesConfig), String> {
    if observed.is_empty() {
        return store.read().await.map(|config| (false, config));
    }
    let observed = observed.clone();
    store
        .update(move |config| {
            let mut changed = false;
            for (lineage, current_key) in observed {
                let previous_key = config
                    .subagent_conflict_lineage_current_keys
                    .get(&lineage)
                    .cloned();
                if previous_key.as_deref() == Some(current_key.as_str()) {
                    continue;
                }
                config
                    .subagent_conflict_lineage_current_keys
                    .insert(lineage, current_key.clone());
                changed = true;
                let previous_choice = previous_key
                    .as_ref()
                    .and_then(|previous_key| config.subagent_conflict_choices.remove(previous_key));
                if previous_choice.is_some() {
                    let replaced = config.subagent_conflict_choices.insert(
                        current_key,
                        SUBAGENT_CONFLICT_RESELECTION_REQUIRED.to_string(),
                    );
                    changed |= replaced.as_deref() != Some(SUBAGENT_CONFLICT_RESELECTION_REQUIRED);
                }
            }
            if changed {
                config.preference_revision = config.preference_revision.saturating_add(1);
            }
            changed
        })
        .await
}

fn merge_subagent_state(
    snapshot: &mut ExternalSourceCatalogSnapshot,
    coordinator_snapshot: &openbitfun_external_sources::ExternalSubagentCoordinatorSnapshot,
    state: &ExternalSubagentProductState,
    preference_revision: u64,
) {
    snapshot.generation = snapshot.generation.max(coordinator_snapshot.generation);
    snapshot.discovery_pending |= coordinator_snapshot.discovery_pending;
    let known_sources = snapshot
        .sources
        .iter()
        .map(|entry| entry.stable_key.clone())
        .collect::<BTreeSet<_>>();
    snapshot.sources.extend(
        coordinator_snapshot
            .sources
            .iter()
            .filter(|source| !known_sources.contains(&source.stable_key))
            .cloned(),
    );
    snapshot
        .sources
        .sort_by(|left, right| left.stable_key.cmp(&right.stable_key));
    snapshot.subagent_generation = coordinator_snapshot.generation;
    snapshot.preference_revision = preference_revision;
    snapshot.subagents = state.summaries.clone();
    snapshot.subagent_model_binding_groups = state.model_binding_groups.clone();
    snapshot.subagent_model_binding_options = state.model_binding_options.clone();
    snapshot.subagent_conflicts = state.conflicts.clone();
    snapshot.pending_subagent_approvals = state.pending_approvals.clone();
    snapshot
        .diagnostics
        .extend(coordinator_snapshot.diagnostics.clone());
}

fn reconcile_observed_tool_conflict(choices: &mut BTreeMap<String, String>, conflict_key: &str) {
    if choices.contains_key(conflict_key) {
        return;
    }
    let Some((lineage, _)) = conflict_key.rsplit_once(':') else {
        choices.insert(
            conflict_key.to_string(),
            UNRESOLVED_TOOL_CONFLICT_CHOICE.to_string(),
        );
        return;
    };
    let requires_fail_closed_reselection = choices.iter().any(|(existing_key, choice)| {
        existing_key
            .rsplit_once(':')
            .is_some_and(|(existing_lineage, _)| existing_lineage == lineage)
            && (choice.starts_with("external:") || choice == TOOL_CONFLICT_RESELECTION_REQUIRED)
    });
    choices.retain(|existing_key, _| {
        existing_key
            .rsplit_once(':')
            .is_none_or(|(existing_lineage, _)| existing_lineage != lineage)
    });
    choices.insert(
        conflict_key.to_string(),
        if requires_fail_closed_reselection {
            TOOL_CONFLICT_RESELECTION_REQUIRED.to_string()
        } else {
            UNRESOLVED_TOOL_CONFLICT_CHOICE.to_string()
        },
    );
}

async fn persist_source_enabled_change(
    stable_key: &str,
    enabled: bool,
    expected_preference_revision: u64,
) -> Result<BTreeSet<String>, String> {
    let stable_key = stable_key.to_string();
    ExternalSourcePreferenceStore::global()?
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return None;
            }
            let mut sources = config
                .suppressed_source_keys
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            if enabled {
                sources.remove(&stable_key);
            } else {
                sources.insert(stable_key);
            }
            let next = sources.iter().cloned().collect::<Vec<_>>();
            if config.suppressed_source_keys != next {
                config.suppressed_source_keys = next;
                config.preference_revision = config.preference_revision.saturating_add(1);
            }
            Some(sources)
        })
        .await
        .and_then(|(sources, _)| {
            sources.ok_or_else(|| {
                stale_operation_error(
                    "External source preferences changed; refresh before retrying",
                )
            })
        })
}

async fn persist_conflict_choice(
    conflict_key: &str,
    candidate_id: &str,
    participants: Vec<String>,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    let conflict_key = conflict_key.to_string();
    let candidate_id = candidate_id.to_string();
    ExternalSourcePreferenceStore::global()?
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return false;
            }
            let previous_choices = config.conflict_choices.clone();
            let previous_lineage = config.conflict_lineage_current_keys.clone();
            let previous_candidates = config.conflicted_candidate_ids.clone();
            ExternalSourceCoordinator::reconcile_conflict_preferences(
                &mut config.conflict_choices,
                &mut config.conflict_lineage_current_keys,
                &mut config.conflicted_candidate_ids,
                &conflict_key,
                &candidate_id,
                &participants,
            );
            if config.conflict_choices != previous_choices
                || config.conflict_lineage_current_keys != previous_lineage
                || config.conflicted_candidate_ids != previous_candidates
            {
                config.preference_revision = config.preference_revision.saturating_add(1);
            }
            true
        })
        .await
        .and_then(|(applied, config)| {
            applied.then_some(config).ok_or_else(|| {
                stale_operation_error(
                    "External command preferences changed; refresh before retrying",
                )
            })
        })
}

async fn persist_tool_target_decision(
    approval_key: &str,
    decision_key: &str,
    approved: bool,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    let approval_key = approval_key.to_string();
    let decision_key = decision_key.to_string();
    ExternalSourcePreferenceStore::global()?
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return false;
            }
            let previous_approved = config.approved_tool_targets.clone();
            let previous_declined = config.declined_tool_decisions.clone();
            reconcile_tool_target_decision(config, approval_key, decision_key, approved);
            if config.approved_tool_targets != previous_approved
                || config.declined_tool_decisions != previous_declined
            {
                config.preference_revision = config.preference_revision.saturating_add(1);
            }
            true
        })
        .await
        .and_then(|(applied, config)| {
            applied.then_some(config).ok_or_else(|| {
                stale_operation_error("External tool preferences changed; refresh before retrying")
            })
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExternalCapabilityBulkDecisionSet {
    Tool(Vec<(String, String)>),
    Subagent(Vec<String>),
    Mcp(Vec<String>),
}

fn apply_capability_bulk_decisions(
    config: &mut ExternalSourcesConfig,
    decisions: ExternalCapabilityBulkDecisionSet,
    enabled: bool,
) -> bool {
    let changed = match decisions {
        ExternalCapabilityBulkDecisionSet::Tool(decisions) => {
            let previous_approved = config.approved_tool_targets.clone();
            let previous_declined = config.declined_tool_decisions.clone();
            for (approval_key, decision_key) in decisions {
                reconcile_tool_target_decision(config, approval_key, decision_key, enabled);
            }
            config.approved_tool_targets != previous_approved
                || config.declined_tool_decisions != previous_declined
        }
        ExternalCapabilityBulkDecisionSet::Subagent(decisions) => {
            let previous_approved = config.approved_subagent_envelopes.clone();
            let previous_declined = config.declined_subagent_decisions.clone();
            for decision_key in decisions {
                if enabled {
                    config
                        .approved_subagent_envelopes
                        .insert(decision_key.clone());
                    config.declined_subagent_decisions.remove(&decision_key);
                } else {
                    config.approved_subagent_envelopes.remove(&decision_key);
                    config
                        .declined_subagent_decisions
                        .insert(decision_key.clone(), decision_key);
                }
            }
            config.approved_subagent_envelopes != previous_approved
                || config.declined_subagent_decisions != previous_declined
        }
        ExternalCapabilityBulkDecisionSet::Mcp(decisions) => {
            let previous = config.mcp_server_decisions.clone();
            for decision_key in decisions {
                reconcile_versioned_mcp_server_decision(
                    &mut config.mcp_server_decisions,
                    decision_key,
                    enabled,
                );
            }
            config.mcp_server_decisions != previous
        }
    };
    if changed {
        config.preference_revision = config.preference_revision.saturating_add(1);
    }
    changed
}

async fn persist_capability_bulk_decisions(
    decisions: ExternalCapabilityBulkDecisionSet,
    enabled: bool,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    ExternalSourcePreferenceStore::global()?
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return false;
            }
            apply_capability_bulk_decisions(config, decisions, enabled);
            true
        })
        .await
        .and_then(|(applied, config)| {
            applied.then_some(config).ok_or_else(|| {
                stale_operation_error(
                    "External capability preferences changed; refresh before retrying",
                )
            })
        })
}

fn reconcile_tool_target_decision(
    config: &mut ExternalSourcesConfig,
    approval_key: String,
    decision_key: String,
    approved: bool,
) {
    if approved {
        config.approved_tool_targets.insert(approval_key.clone());
        config.declined_tool_decisions.remove(&approval_key);
    } else {
        config.approved_tool_targets.remove(&approval_key);
        config
            .declined_tool_decisions
            .insert(approval_key, decision_key);
    }
}

async fn persist_tool_conflict_choice(
    conflict_key: &str,
    candidate_id: &str,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    let conflict_key = conflict_key.to_string();
    let candidate_id = candidate_id.to_string();
    ExternalSourcePreferenceStore::global()?
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return false;
            }
            let previous = config.tool_conflict_choices.clone();
            reconcile_versioned_tool_conflict_choice(
                &mut config.tool_conflict_choices,
                conflict_key,
                candidate_id,
            );
            if config.tool_conflict_choices != previous {
                config.preference_revision = config.preference_revision.saturating_add(1);
            }
            true
        })
        .await
        .and_then(|(applied, config)| {
            applied.then_some(config).ok_or_else(|| {
                stale_operation_error("External tool preferences changed; refresh before retrying")
            })
        })
}

async fn persist_subagent_activation(
    approval_key: &str,
    approved: bool,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    let approval_key = approval_key.to_string();
    ExternalSourcePreferenceStore::global()?
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return false;
            }
            if approved {
                config
                    .approved_subagent_envelopes
                    .insert(approval_key.clone());
                config.declined_subagent_decisions.remove(&approval_key);
            } else {
                config.approved_subagent_envelopes.remove(&approval_key);
                config
                    .declined_subagent_decisions
                    .insert(approval_key.clone(), approval_key);
            }
            config.preference_revision = config.preference_revision.saturating_add(1);
            true
        })
        .await
        .and_then(|(applied, config)| {
            applied.then_some(config).ok_or_else(|| {
                stale_operation_error(
                    "External subagent preferences changed; refresh before retrying",
                )
            })
        })
}

async fn persist_subagent_conflict_choice(
    conflict_key: &str,
    candidate_id: &str,
    approval_key: Option<&str>,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    let store = ExternalSourcePreferenceStore::global()?;
    persist_subagent_conflict_choice_with_store(
        &store,
        conflict_key,
        candidate_id,
        approval_key,
        expected_preference_revision,
    )
    .await
}

async fn persist_subagent_conflict_choice_with_store(
    store: &ExternalSourcePreferenceStore,
    conflict_key: &str,
    candidate_id: &str,
    approval_key: Option<&str>,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    let conflict_key = conflict_key.to_string();
    let candidate_id = candidate_id.to_string();
    let approval_key = approval_key.map(str::to_string);
    store
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return false;
            }
            config
                .subagent_conflict_choices
                .insert(conflict_key, candidate_id);
            if let Some(approval_key) = approval_key {
                config
                    .approved_subagent_envelopes
                    .insert(approval_key.clone());
                config.declined_subagent_decisions.remove(&approval_key);
            }
            config.preference_revision = config.preference_revision.saturating_add(1);
            true
        })
        .await
        .and_then(|(applied, config)| {
            applied.then_some(config).ok_or_else(|| {
                stale_operation_error(
                    "External subagent preferences changed; refresh before retrying",
                )
            })
        })
}

fn validate_subagent_model_binding_mutation(
    snapshot: &ExternalSourceCatalogSnapshot,
    binding_key: &str,
    target: Option<&ExternalSubagentModelBindingTarget>,
    expected_subagent_generation: u64,
    expected_preference_revision: u64,
) -> Result<(), String> {
    if snapshot.subagent_generation != expected_subagent_generation
        || snapshot.preference_revision != expected_preference_revision
    {
        return Err(stale_operation_error(
            "External subagent catalog changed; refresh before retrying",
        ));
    }
    let group = snapshot
        .subagent_model_binding_groups
        .iter()
        .find(|group| group.binding_key == binding_key)
        .ok_or_else(|| {
            missing_candidate_error("External subagent model binding is no longer available")
        })?;
    if !matches!(
        group.method,
        ExternalSubagentModelBindingMethod::BindingRequired
            | ExternalSubagentModelBindingMethod::Explicit
            | ExternalSubagentModelBindingMethod::BindingUnavailable
    ) {
        return Err(unavailable_operation_error(
            "External subagent model binding is read-only in its current state",
        ));
    }
    if let Some(target) = target {
        if !snapshot
            .subagent_model_binding_options
            .iter()
            .any(|option| &option.target == target)
        {
            return Err(unavailable_operation_error(
                "External subagent model binding target is unavailable",
            ));
        }
    }
    Ok(())
}

async fn persist_subagent_model_binding(
    binding_key: &str,
    target: Option<ExternalSubagentModelBindingTarget>,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    let store = ExternalSourcePreferenceStore::global()?;
    persist_subagent_model_binding_with_store(
        &store,
        binding_key,
        target,
        expected_preference_revision,
    )
    .await
}

async fn persist_subagent_model_binding_with_store(
    store: &ExternalSourcePreferenceStore,
    binding_key: &str,
    target: Option<ExternalSubagentModelBindingTarget>,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    let binding_key = binding_key.to_string();
    store
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return false;
            }
            match target {
                Some(target) => {
                    config.subagent_model_bindings.insert(binding_key, target);
                }
                None => {
                    config.subagent_model_bindings.remove(&binding_key);
                }
            }
            config.preference_revision = config.preference_revision.saturating_add(1);
            true
        })
        .await
        .and_then(|(applied, config)| {
            applied.then_some(config).ok_or_else(|| {
                stale_operation_error(
                    "External subagent preferences changed; refresh before retrying",
                )
            })
        })
}

async fn persist_mcp_server_decision(
    decision_key: &str,
    approved: bool,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    let store = ExternalSourcePreferenceStore::global()?;
    let decision_key = decision_key.to_string();
    store
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return false;
            }
            reconcile_versioned_mcp_server_decision(
                &mut config.mcp_server_decisions,
                decision_key,
                approved,
            );
            config.preference_revision = config.preference_revision.saturating_add(1);
            true
        })
        .await
        .and_then(|(applied, config)| {
            applied.then_some(config).ok_or_else(|| {
                stale_operation_error("External MCP preferences changed; refresh before retrying")
            })
        })
}

async fn persist_mcp_conflict_choice(
    conflict_key: &str,
    candidate_id: &str,
    external_decision: Option<&str>,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    let store = ExternalSourcePreferenceStore::global()?;
    let conflict_key = conflict_key.to_string();
    let candidate_id = candidate_id.to_string();
    let external_decision = external_decision.map(str::to_string);
    store
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return false;
            }
            reconcile_versioned_mcp_conflict_choice(
                &mut config.mcp_conflict_choices,
                conflict_key,
                candidate_id,
            );
            if let Some(decision_key) = external_decision {
                reconcile_versioned_mcp_server_decision(
                    &mut config.mcp_server_decisions,
                    decision_key,
                    true,
                );
            }
            config.preference_revision = config.preference_revision.saturating_add(1);
            true
        })
        .await
        .and_then(|(applied, config)| {
            applied.then_some(config).ok_or_else(|| {
                stale_operation_error("External MCP preferences changed; refresh before retrying")
            })
        })
}

fn validate_integration_policy_operation(
    scope: ExternalIntegrationPolicyScope,
    operation: &ExternalIntegrationPolicyOperation,
) -> Result<(), String> {
    let descriptors = default_external_integration_ecosystems();
    let validate_ecosystem =
        |ecosystem_id: &openbitfun_product_domains::external_sources::EcosystemId| {
            descriptors
                .iter()
                .find(|descriptor| descriptor.ecosystem_id == *ecosystem_id)
                .ok_or_else(|| {
                    invalid_operation_error(format!(
                        "External ecosystem '{}' is not registered",
                        ecosystem_id
                    ))
                })
        };
    match operation {
        ExternalIntegrationPolicyOperation::SetEnabled { .. }
        | ExternalIntegrationPolicyOperation::SetAutomaticDiscovery { .. } => Ok(()),
        ExternalIntegrationPolicyOperation::SetEcosystemMode { ecosystem_id, mode } => {
            validate_ecosystem(ecosystem_id)?;
            mode.is_known().then_some(()).ok_or_else(|| {
                invalid_operation_error("External integration mode is not supported")
            })
        }
        ExternalIntegrationPolicyOperation::SetCapabilityAccess {
            ecosystem_id,
            capability_id,
            access,
        } => {
            let descriptor = validate_ecosystem(ecosystem_id)?;
            if !descriptor
                .capabilities
                .iter()
                .any(|capability| capability.capability_id == *capability_id)
                || !access.is_known()
            {
                return Err(invalid_operation_error(format!(
                    "External capability '{}' is not registered",
                    capability_id
                )));
            }
            Ok(())
        }
        ExternalIntegrationPolicyOperation::ResetWorkspace
            if scope == ExternalIntegrationPolicyScope::Workspace =>
        {
            Ok(())
        }
        ExternalIntegrationPolicyOperation::ResetWorkspace => Err(invalid_operation_error(
            "reset_workspace requires workspace policy scope",
        )),
        ExternalIntegrationPolicyOperation::ResetIncompatiblePolicy
            if scope == ExternalIntegrationPolicyScope::User =>
        {
            Ok(())
        }
        ExternalIntegrationPolicyOperation::ResetIncompatiblePolicy => Err(
            invalid_operation_error("reset_incompatible_policy requires user policy scope"),
        ),
        _ => Err(invalid_operation_error("Policy operation is not supported")),
    }
}

fn apply_user_policy_operation(
    settings: &mut ExternalIntegrationPolicySettings,
    operation: &ExternalIntegrationPolicyOperation,
) -> Result<bool, String> {
    match operation {
        ExternalIntegrationPolicyOperation::SetAutomaticDiscovery { enabled } => {
            let changed = settings.automatic_discovery != Some(*enabled);
            settings.automatic_discovery = Some(*enabled);
            Ok(changed)
        }
        ExternalIntegrationPolicyOperation::SetEnabled { enabled } => {
            let changed = settings.enabled != *enabled;
            settings.enabled = *enabled;
            Ok(changed)
        }
        ExternalIntegrationPolicyOperation::SetEcosystemMode { ecosystem_id, mode } => {
            let policy = settings.ecosystems.entry(ecosystem_id.clone()).or_default();
            let changed = policy.mode != *mode;
            policy.mode = mode.clone();
            Ok(changed)
        }
        ExternalIntegrationPolicyOperation::SetCapabilityAccess {
            ecosystem_id,
            capability_id,
            access,
        } => {
            let policy = settings.ecosystems.entry(ecosystem_id.clone()).or_default();
            let changed = policy.mode != ExternalIntegrationMode::Custom
                || policy.capability_overrides.get(capability_id) != Some(access);
            policy.mode = ExternalIntegrationMode::Custom;
            policy
                .capability_overrides
                .insert(capability_id.clone(), access.clone());
            Ok(changed)
        }
        ExternalIntegrationPolicyOperation::ResetWorkspace => Err(invalid_operation_error(
            "reset_workspace cannot update user defaults",
        )),
        _ => Err(invalid_operation_error("Policy operation is not supported")),
    }
}

fn apply_workspace_policy_operation(
    document: &mut ExternalIntegrationPolicyDocument,
    workspace_key: &str,
    operation: &ExternalIntegrationPolicyOperation,
) -> Result<bool, String> {
    if matches!(
        operation,
        ExternalIntegrationPolicyOperation::ResetWorkspace
    ) {
        return Ok(document.workspace_overrides.remove(workspace_key).is_some());
    }
    let policy = document
        .workspace_overrides
        .entry(workspace_key.to_string())
        .or_default();
    match operation {
        ExternalIntegrationPolicyOperation::SetAutomaticDiscovery { enabled } => {
            let changed = policy.automatic_discovery != Some(*enabled);
            policy.automatic_discovery = Some(*enabled);
            Ok(changed)
        }
        ExternalIntegrationPolicyOperation::SetEnabled { enabled } => {
            let changed = policy.enabled != Some(*enabled);
            policy.enabled = Some(*enabled);
            Ok(changed)
        }
        ExternalIntegrationPolicyOperation::SetEcosystemMode { ecosystem_id, mode } => {
            let ecosystem = policy.ecosystems.entry(ecosystem_id.clone()).or_default();
            let changed = ecosystem.mode.as_ref() != Some(mode);
            ecosystem.mode = Some(mode.clone());
            Ok(changed)
        }
        ExternalIntegrationPolicyOperation::SetCapabilityAccess {
            ecosystem_id,
            capability_id,
            access,
        } => {
            let ecosystem = policy.ecosystems.entry(ecosystem_id.clone()).or_default();
            let changed = ecosystem.mode != Some(ExternalIntegrationMode::Custom)
                || ecosystem.capability_overrides.get(capability_id) != Some(access);
            ecosystem.mode = Some(ExternalIntegrationMode::Custom);
            ecosystem
                .capability_overrides
                .insert(capability_id.clone(), access.clone());
            Ok(changed)
        }
        ExternalIntegrationPolicyOperation::ResetWorkspace => Ok(false),
        _ => Err(invalid_operation_error("Policy operation is not supported")),
    }
}

async fn persist_integration_policy_mutation(
    workspace_id: Option<&str>,
    mutation: ExternalIntegrationPolicyMutation,
) -> Result<ExternalSourcesConfig, String> {
    validate_integration_policy_operation(mutation.scope, &mutation.change)?;
    let workspace_key = match mutation.scope {
        ExternalIntegrationPolicyScope::User => None,
        ExternalIntegrationPolicyScope::Workspace => {
            Some(workspace_policy_key(workspace_id).ok_or_else(|| {
                invalid_operation_error("Workspace policy scope requires a workspace")
            })?)
        }
        _ => {
            return Err(invalid_operation_error("Policy scope is not supported"));
        }
    };
    ExternalSourcePreferenceStore::global()?
        .update(move |config| {
            apply_integration_policy_mutation_to_config(config, workspace_key.as_deref(), &mutation)
                .map(|_| ())
        })
        .await
        .and_then(|(result, config)| result.map(|()| config))
}

fn apply_integration_policy_mutation_to_config(
    config: &mut ExternalSourcesConfig,
    workspace_key: Option<&str>,
    mutation: &ExternalIntegrationPolicyMutation,
) -> Result<bool, String> {
    if config.preference_revision != mutation.expected_preference_revision {
        return Err(stale_operation_error(
            "External integration policy changed; refresh before retrying",
        ));
    }
    let incompatible = config.integration_policy.known().is_none();
    if incompatible {
        if mutation.scope == ExternalIntegrationPolicyScope::User
            && matches!(
                &mutation.change,
                ExternalIntegrationPolicyOperation::ResetIncompatiblePolicy
            )
        {
            config
                .integration_policy_backups
                .push(config.integration_policy.raw_value());
            const MAX_POLICY_BACKUPS: usize = 3;
            if config.integration_policy_backups.len() > MAX_POLICY_BACKUPS {
                let remove_count = config.integration_policy_backups.len() - MAX_POLICY_BACKUPS;
                config.integration_policy_backups.drain(0..remove_count);
            }
            let mut reset_policy = StoredExternalIntegrationPolicy::default();
            reset_policy
                .known_mut()
                .expect("the host-owned default policy schema must be compatible")
                .user_defaults
                .enabled = false;
            config.integration_policy = reset_policy;
            config.preference_revision = config.preference_revision.saturating_add(1);
            return Ok(true);
        }
        return Err(incompatible_policy_error(format!(
            "External integration policy schema {} is not supported; back up and reset it before making changes",
            config.integration_policy.schema_major()
        )));
    }
    if matches!(
        &mutation.change,
        ExternalIntegrationPolicyOperation::ResetIncompatiblePolicy
    ) {
        return Err(invalid_operation_error(
            "External integration policy is already compatible",
        ));
    }
    let document = config.integration_policy.known_mut().ok_or_else(|| {
        incompatible_policy_error("External integration policy requires a backup and reset")
    })?;
    let changed = match mutation.scope {
        ExternalIntegrationPolicyScope::User => {
            apply_user_policy_operation(&mut document.user_defaults, &mutation.change)?
        }
        ExternalIntegrationPolicyScope::Workspace => apply_workspace_policy_operation(
            document,
            workspace_key.ok_or_else(|| {
                invalid_operation_error("Workspace policy scope requires a workspace")
            })?,
            &mutation.change,
        )?,
        _ => return Err(invalid_operation_error("Policy scope is not supported")),
    };
    if changed {
        config.preference_revision = config.preference_revision.saturating_add(1);
    }
    Ok(changed)
}

fn reconcile_versioned_mcp_conflict_choice(
    choices: &mut BTreeMap<String, String>,
    conflict_key: String,
    candidate_id: String,
) {
    if let Some((lineage, _)) = conflict_key.rsplit_once(':') {
        choices.retain(|existing_key, _| {
            existing_key
                .rsplit_once(':')
                .is_none_or(|(existing_lineage, _)| existing_lineage != lineage)
        });
    }
    choices.insert(conflict_key, candidate_id);
}

fn reconcile_versioned_mcp_server_decision(
    decisions: &mut BTreeMap<String, ExternalMcpDecision>,
    decision_key: String,
    approved: bool,
) {
    if let Some((lineage, _)) = decision_key.rsplit_once(':') {
        decisions.retain(|existing_key, _| {
            existing_key
                .rsplit_once(':')
                .is_none_or(|(existing_lineage, _)| existing_lineage != lineage)
        });
    }
    decisions.insert(
        decision_key.clone(),
        ExternalMcpDecision {
            decision_key,
            approved,
        },
    );
}

fn reconcile_versioned_tool_conflict_choice(
    choices: &mut BTreeMap<String, String>,
    conflict_key: String,
    candidate_id: String,
) {
    if let Some((lineage, _)) = conflict_key.rsplit_once(':') {
        choices.retain(|existing_key, _| {
            existing_key
                .rsplit_once(':')
                .is_none_or(|(existing_lineage, _)| existing_lineage != lineage)
        });
    }
    choices.insert(conflict_key, candidate_id);
}

fn propagate_suppressed_sources(
    sources: &BTreeSet<String>,
    current: &Arc<WorkspaceExternalSourceService>,
) {
    for service in workspace_services().iter() {
        let Some(service) = service.value().upgrade() else {
            continue;
        };
        if Arc::ptr_eq(&service, current) {
            continue;
        }
        lock_coordinator(&service.control_plane).replace_suppressed_sources(sources.clone());
        lock_tool_coordinator(&service.control_plane).replace_suppressed_sources(sources.clone());
        lock_subagent_coordinator(&service.control_plane)
            .replace_suppressed_sources(sources.clone());
        lock_mcp_coordinator(&service.control_plane).replace_suppressed_sources(sources.clone());
        lock_workspace_reference_coordinator(&service.control_plane)
            .replace_suppressed_sources(sources.clone());
        tokio::spawn(async move {
            if let Err(error) = service.refresh_preserving_worker_recovery().await {
                log::warn!(
                    "Could not refresh external sources after source preference change scope={} error_category={}",
                    external_log_scope(service.workspace_root.as_deref()),
                    external_log_error_category(&error),
                );
            }
        });
    }
}

fn propagate_conflict_preferences(preferences: &ExternalSourcesConfig) {
    for service in workspace_services().iter() {
        let Some(service) = service.value().upgrade() else {
            continue;
        };
        {
            let mut coordinator = lock_coordinator(&service.control_plane);
            coordinator.replace_conflict_choices(preferences.conflict_choices.clone());
            coordinator.replace_conflict_lineage_current_keys(
                preferences.conflict_lineage_current_keys.clone(),
            );
            coordinator
                .replace_conflicted_candidate_ids(preferences.conflicted_candidate_ids.clone());
        }
        tokio::spawn(async move {
            let command_snapshot = lock_coordinator(&service.control_plane).snapshot();
            if let Ok(snapshot) = service.rebuild_product_snapshot(command_snapshot).await {
                let _ = service.updates.send(snapshot);
            }
        });
    }
}

fn propagate_tool_preferences(_preferences: &ExternalSourcesConfig) {
    propagate_runtime_preference_revision();
}

fn propagate_prompt_command_preferences(_preferences: &ExternalSourcesConfig) {
    propagate_runtime_preference_revision();
}

fn propagate_runtime_preference_revision() {
    for service in workspace_services().iter() {
        let Some(service) = service.value().upgrade() else {
            continue;
        };
        tokio::spawn(async move {
            let command_snapshot = lock_coordinator(&service.control_plane).snapshot();
            if let Ok(snapshot) = service.rebuild_product_snapshot(command_snapshot).await {
                let _ = service.updates.send(snapshot);
            }
        });
    }
}

fn propagate_subagent_preferences(_preferences: &ExternalSourcesConfig) {
    propagate_runtime_preference_revision();
}

fn propagate_mcp_preferences(_preferences: &ExternalSourcesConfig) {
    propagate_runtime_preference_revision();
}

fn propagate_integration_policy_preferences(
    _preferences: &ExternalSourcesConfig,
    current: &Arc<WorkspaceExternalSourceService>,
) {
    for service in workspace_services().iter() {
        let Some(service) = service.value().upgrade() else {
            continue;
        };
        if Arc::ptr_eq(&service, current) {
            continue;
        }
        tokio::spawn(async move {
            if let Err(error) = service.refresh_preserving_worker_recovery().await {
                log::warn!(
                    "Could not apply external integration policy update scope={} error_category={}",
                    external_log_scope(service.workspace_root.as_deref()),
                    external_log_error_category(&error),
                );
            }
        });
    }
}

pub(crate) fn notify_external_tool_registry_changed() {
    TOOL_REGISTRY_CHANGE_EPOCH.fetch_add(1, Ordering::AcqRel);
    if TOOL_REGISTRY_REBUILD_SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        TOOL_REGISTRY_REBUILD_SCHEDULED.store(false, Ordering::Release);
        return;
    };
    runtime.spawn(async move {
        loop {
            let observed_epoch = TOOL_REGISTRY_CHANGE_EPOCH.load(Ordering::Acquire);
            let services = workspace_services()
                .iter()
                .filter_map(|entry| entry.value().upgrade())
                .collect::<Vec<_>>();
            for service in services {
                let command_snapshot = lock_coordinator(&service.control_plane).snapshot();
                if let Ok(snapshot) = service.rebuild_product_snapshot(command_snapshot).await {
                    let _ = service.updates.send(snapshot);
                }
            }
            if TOOL_REGISTRY_CHANGE_EPOCH.load(Ordering::Acquire) != observed_epoch {
                continue;
            }
            TOOL_REGISTRY_REBUILD_SCHEDULED.store(false, Ordering::Release);
            if TOOL_REGISTRY_CHANGE_EPOCH.load(Ordering::Acquire) == observed_epoch {
                break;
            }
            if TOOL_REGISTRY_REBUILD_SCHEDULED.swap(true, Ordering::AcqRel) {
                break;
            }
        }
    });
}

async fn sync_service_preferences(service: &WorkspaceExternalSourceService) -> Result<(), String> {
    let preferences = read_external_sources_config().await?;
    let policy = integration_policy_snapshot(&preferences, service.workspace_id.as_deref())?;
    let suppressed_sources = preferences
        .suppressed_source_keys
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let command_changed = {
        let mut coordinator = lock_coordinator(&service.control_plane);
        let mut changed = false;
        if coordinator.suppressed_sources() != &suppressed_sources {
            coordinator.replace_suppressed_sources(suppressed_sources.clone());
            changed = true;
        }
        if coordinator.conflict_choices() != &preferences.conflict_choices {
            coordinator.replace_conflict_choices(preferences.conflict_choices.clone());
            changed = true;
        }
        if coordinator.conflict_lineage_current_keys() != &preferences.conflict_lineage_current_keys
        {
            coordinator.replace_conflict_lineage_current_keys(
                preferences.conflict_lineage_current_keys.clone(),
            );
            changed = true;
        }
        if coordinator.conflicted_candidate_ids() != &preferences.conflicted_candidate_ids {
            coordinator
                .replace_conflicted_candidate_ids(preferences.conflicted_candidate_ids.clone());
            changed = true;
        }
        changed
    };
    let tool_changed = {
        let mut coordinator = lock_tool_coordinator(&service.control_plane);
        if coordinator.suppressed_sources() != &suppressed_sources {
            coordinator.replace_suppressed_sources(suppressed_sources.clone());
            true
        } else {
            false
        }
    };
    let subagent_changed = {
        let mut coordinator = lock_subagent_coordinator(&service.control_plane);
        if coordinator.suppressed_sources() != &suppressed_sources {
            coordinator.replace_suppressed_sources(suppressed_sources.clone());
            true
        } else {
            false
        }
    };
    let mcp_changed = {
        let mut coordinator = lock_mcp_coordinator(&service.control_plane);
        if coordinator.suppressed_sources() != &suppressed_sources {
            coordinator.replace_suppressed_sources(suppressed_sources.clone());
            true
        } else {
            false
        }
    };
    let workspace_reference_changed = {
        let mut coordinator = lock_workspace_reference_coordinator(&service.control_plane);
        if coordinator.suppressed_sources() != &suppressed_sources {
            coordinator.replace_suppressed_sources(suppressed_sources.clone());
            true
        } else {
            false
        }
    };
    let subagent_preferences_changed =
        service.snapshot().preference_revision != preferences.preference_revision;
    let policy_changed = service.snapshot().integration_policy != policy;
    if command_changed
        || tool_changed
        || subagent_changed
        || mcp_changed
        || workspace_reference_changed
        || subagent_preferences_changed
        || policy_changed
    {
        let command_snapshot = lock_coordinator(&service.control_plane).snapshot();
        let snapshot = service.rebuild_product_snapshot(command_snapshot).await?;
        let _ = service.updates.send(snapshot);
    }
    service.ensure_watch_roots(&policy).await;
    Ok(())
}

fn validate_conflict_preference(conflict_key: &str, candidate_id: &str) -> Result<(), String> {
    if conflict_key.is_empty() || conflict_key.len() > 512 {
        return Err(invalid_operation_error(
            "External source conflict key is invalid",
        ));
    }
    if candidate_id.is_empty() || candidate_id.len() > 512 {
        return Err(invalid_operation_error(
            "External source conflict candidate is invalid",
        ));
    }
    Ok(())
}

fn validate_subagent_decision_value(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(invalid_operation_error(format!(
            "External subagent {label} is invalid"
        )));
    }
    Ok(())
}

fn validate_mcp_decision_value(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(invalid_operation_error(format!(
            "External MCP {label} is invalid"
        )));
    }
    Ok(())
}

pub(super) fn external_mcp_decision_allowed(
    state: &ExternalMcpActivationState,
    approved: bool,
) -> bool {
    if matches!(
        state,
        ExternalMcpActivationState::Conflict
            | ExternalMcpActivationState::Covered { .. }
            | ExternalMcpActivationState::SourceDisabled
            | ExternalMcpActivationState::Unsupported { .. }
            | ExternalMcpActivationState::Removed
    ) {
        return false;
    }
    !approved
        || !matches!(
            state,
            ExternalMcpActivationState::Starting
                | ExternalMcpActivationState::RuntimeUnavailable { .. }
        )
}

fn project_native_prompt_command_conflicts(
    snapshot: &ExternalSourceCatalogSnapshot,
    native_commands: &[NativePromptCommandDescriptor],
    conflict_choices: &BTreeMap<String, String>,
    conflicted_candidate_ids: &BTreeSet<String>,
    preference_revision: u64,
) -> Result<NativePromptCommandConflictSnapshot, String> {
    for command in native_commands {
        command
            .validate()
            .map_err(|error| invalid_operation_error(error.to_string()))?;
    }
    let command_names = native_commands
        .iter()
        .map(|command| command.command_name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let mut projected = Vec::new();
    let mut reconfirmations = Vec::new();
    for command_name in &command_names {
        let native = native_commands
            .iter()
            .filter(|command| command.command_name.eq_ignore_ascii_case(command_name))
            .collect::<Vec<_>>();
        if native.is_empty() {
            continue;
        }
        let unresolved_conflict = snapshot.command_conflicts.iter().find(|conflict| {
            conflict.selected_candidate_id.is_none()
                && conflict.command_name.eq_ignore_ascii_case(command_name)
        });
        let external = if let Some(conflict) = unresolved_conflict {
            conflict
                .candidates
                .iter()
                .map(|candidate| {
                    (
                        candidate.candidate_id.clone(),
                        candidate.content_version.clone(),
                        candidate.source.clone(),
                    )
                })
                .collect::<Vec<_>>()
        } else {
            snapshot
                .commands
                .iter()
                .filter(|entry| entry.definition.name.eq_ignore_ascii_case(command_name))
                .map(|entry| {
                    (
                        entry.definition.id.stable_key(),
                        entry.definition.content_version.clone(),
                        entry.definition.id.source.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let Some((_, _, source)) = external.first() else {
            reconfirmations.extend(
                native
                    .iter()
                    .filter(|&command| conflicted_candidate_ids.contains(&command.candidate_id))
                    .map(|command| NativePromptCommandReconfirmationProjection {
                        command_name: command_name.clone(),
                        native_candidate_id: command.candidate_id.clone(),
                    }),
            );
            continue;
        };
        let execution_domain = snapshot
            .sources
            .iter()
            .find(|entry| &entry.record.key == source)
            .map(|entry| entry.record.execution_domain_id.as_str())
            .ok_or_else(|| invalid_operation_error("External command source is unavailable"))?;
        let mut participants = native
            .iter()
            .map(|command| {
                (
                    command.candidate_id.as_str(),
                    command.behavior_version.as_str(),
                )
            })
            .collect::<Vec<_>>();
        participants.extend(
            external
                .iter()
                .map(|(candidate_id, version, _)| (candidate_id.as_str(), version.as_str())),
        );
        let conflict_key =
            native_prompt_command_conflict_key(execution_domain, command_name, participants);
        let selected_candidate_id = conflict_choices.get(&conflict_key).cloned();
        projected.extend(external.into_iter().map(|(candidate_id, _, _)| {
            NativePromptCommandConflictProjection {
                command_name: command_name.clone(),
                external_candidate_id: candidate_id,
                conflict_key: conflict_key.clone(),
                selected_candidate_id: selected_candidate_id.clone(),
            }
        }));
    }
    projected.sort_by(|left, right| {
        (&left.command_name, &left.external_candidate_id)
            .cmp(&(&right.command_name, &right.external_candidate_id))
    });
    reconfirmations.sort_by(|left, right| {
        (&left.command_name, &left.native_candidate_id)
            .cmp(&(&right.command_name, &right.native_candidate_id))
    });
    Ok(NativePromptCommandConflictSnapshot {
        preference_revision,
        conflicts: projected,
        reconfirmations,
    })
}

pub async fn native_prompt_command_conflicts(
    workspace_id: Option<&str>,
    native_commands: Vec<NativePromptCommandDescriptor>,
) -> Result<NativePromptCommandConflictSnapshot, String> {
    let snapshot = external_source_snapshot(workspace_id, false).await?;
    let preferences = read_external_sources_config().await?;
    project_native_prompt_command_conflicts(
        &snapshot,
        &native_commands,
        &preferences.conflict_choices,
        &preferences.conflicted_candidate_ids,
        preferences.preference_revision,
    )
}

pub async fn set_native_prompt_command_conflict_choice(
    workspace_id: Option<&str>,
    native_commands: Vec<NativePromptCommandDescriptor>,
    selected_candidate_id: &str,
    expected_preference_revision: u64,
) -> Result<NativePromptCommandConflictSnapshot, String> {
    let snapshot = external_source_snapshot(workspace_id, false).await?;
    let preferences = read_external_sources_config().await?;
    if preferences.preference_revision != expected_preference_revision {
        return Err(stale_operation_error(
            "External source preferences changed before the command choice was saved",
        ));
    }
    let projection = project_native_prompt_command_conflicts(
        &snapshot,
        &native_commands,
        &preferences.conflict_choices,
        &preferences.conflicted_candidate_ids,
        preferences.preference_revision,
    )?;
    let selected_projection = projection.conflicts.iter().find(|conflict| {
        conflict.external_candidate_id == selected_candidate_id
            || native_commands.iter().any(|command| {
                command
                    .command_name
                    .eq_ignore_ascii_case(&conflict.command_name)
                    && command.candidate_id == selected_candidate_id
            })
    });
    if selected_projection.is_none() {
        let reconfirmation = projection
            .reconfirmations
            .iter()
            .find(|item| item.native_candidate_id == selected_candidate_id);
        if let Some(reconfirmation) = reconfirmation {
            let native_candidate_ids = native_commands
                .iter()
                .filter(|command| {
                    command
                        .command_name
                        .eq_ignore_ascii_case(&reconfirmation.command_name)
                })
                .map(|command| command.candidate_id.clone())
                .collect::<Vec<_>>();
            let preferences = confirm_native_prompt_command_reconfirmation(
                &reconfirmation.command_name,
                native_candidate_ids,
                expected_preference_revision,
            )
            .await?;
            return project_native_prompt_command_conflicts(
                &snapshot,
                &native_commands,
                &preferences.conflict_choices,
                &preferences.conflicted_candidate_ids,
                preferences.preference_revision,
            );
        }
    }
    let Some(selected_projection) = selected_projection else {
        return Err(invalid_operation_error(
            "The selected native command conflict candidate is no longer available",
        ));
    };
    let mut participants = projection
        .conflicts
        .iter()
        .filter(|conflict| conflict.conflict_key == selected_projection.conflict_key)
        .map(|conflict| conflict.external_candidate_id.clone())
        .collect::<Vec<_>>();
    participants.extend(
        native_commands
            .iter()
            .filter(|command| {
                command
                    .command_name
                    .eq_ignore_ascii_case(&selected_projection.command_name)
            })
            .map(|command| command.candidate_id.clone()),
    );
    participants.sort();
    participants.dedup();
    let native_candidate_ids = native_commands
        .iter()
        .filter(|command| {
            command
                .command_name
                .eq_ignore_ascii_case(&selected_projection.command_name)
        })
        .map(|command| command.candidate_id.clone())
        .collect::<Vec<_>>();
    let (choices, _, conflicted_candidate_ids, preference_revision) =
        remember_native_prompt_command_conflict_choice(
            &selected_projection.conflict_key,
            selected_candidate_id,
            participants,
            native_candidate_ids,
            expected_preference_revision,
        )
        .await?;
    project_native_prompt_command_conflicts(
        &snapshot,
        &native_commands,
        &choices,
        &conflicted_candidate_ids,
        preference_revision,
    )
}

pub async fn external_source_conflict_choices() -> Result<
    (
        BTreeMap<String, String>,
        BTreeMap<String, String>,
        BTreeSet<String>,
    ),
    String,
> {
    let preferences = read_external_sources_config().await?;
    Ok((
        preferences.conflict_choices,
        preferences.conflict_lineage_current_keys,
        preferences.conflicted_candidate_ids,
    ))
}

async fn remember_native_prompt_command_conflict_choice(
    conflict_key: &str,
    candidate_id: &str,
    participants: Vec<String>,
    native_candidate_ids: Vec<String>,
    expected_preference_revision: u64,
) -> Result<
    (
        BTreeMap<String, String>,
        BTreeMap<String, String>,
        BTreeSet<String>,
        u64,
    ),
    String,
> {
    validate_conflict_preference(conflict_key, candidate_id)?;
    if native_prompt_command_conflict_key_command(conflict_key).is_none()
        || participants.is_empty()
        || native_candidate_ids.is_empty()
        || !participants
            .iter()
            .any(|candidate| candidate == candidate_id)
        || participants
            .iter()
            .any(|candidate| validate_conflict_preference(conflict_key, candidate).is_err())
        || native_candidate_ids.iter().any(|candidate| {
            !participants.contains(candidate)
                || !candidate.starts_with("openbitfun.")
                || validate_conflict_preference(conflict_key, candidate).is_err()
        })
    {
        return Err(invalid_operation_error(
            "Native prompt command conflict participants are invalid",
        ));
    }

    let conflict_key = conflict_key.to_string();
    let candidate_id = candidate_id.to_string();
    let persisted = ExternalSourcePreferenceStore::global()?
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return false;
            }
            let changed = reconcile_native_prompt_command_conflict_preference(
                config,
                &conflict_key,
                &candidate_id,
                &native_candidate_ids,
            );
            if changed {
                config.preference_revision = config.preference_revision.saturating_add(1);
            }
            true
        })
        .await
        .and_then(|(applied, config)| {
            applied.then_some(config).ok_or_else(|| {
                stale_operation_error(
                    "External command preferences changed; refresh before retrying",
                )
            })
        })?;
    propagate_conflict_preferences(&persisted);
    Ok((
        persisted.conflict_choices,
        persisted.conflict_lineage_current_keys,
        persisted.conflicted_candidate_ids,
        persisted.preference_revision,
    ))
}

fn reconcile_native_prompt_command_conflict_preference(
    config: &mut ExternalSourcesConfig,
    conflict_key: &str,
    candidate_id: &str,
    native_candidate_ids: &[String],
) -> bool {
    let previous_choices = config.conflict_choices.clone();
    let previous_candidates = config.conflicted_candidate_ids.clone();
    let lineage = conflict_key.rsplit_once(':').map(|(lineage, _)| lineage);
    config.conflict_choices.retain(|key, _| {
        !key.starts_with("native:prompt_command:")
            || lineage.is_none_or(|lineage| {
                key.rsplit_once(':')
                    .is_none_or(|(candidate_lineage, _)| candidate_lineage != lineage)
            })
    });
    config
        .conflict_choices
        .insert(conflict_key.to_string(), candidate_id.to_string());
    config
        .conflicted_candidate_ids
        .extend(native_candidate_ids.iter().cloned());
    config.conflict_choices != previous_choices
        || config.conflicted_candidate_ids != previous_candidates
}

async fn confirm_native_prompt_command_reconfirmation(
    command_name: &str,
    native_candidate_ids: Vec<String>,
    expected_preference_revision: u64,
) -> Result<ExternalSourcesConfig, String> {
    let command_name = command_name.to_ascii_lowercase();
    let persisted = ExternalSourcePreferenceStore::global()?
        .update(move |config| {
            if config.preference_revision != expected_preference_revision {
                return false;
            }
            if reconcile_native_prompt_command_reconfirmation(
                config,
                &command_name,
                &native_candidate_ids,
            ) {
                config.preference_revision = config.preference_revision.saturating_add(1);
            }
            true
        })
        .await
        .and_then(|(applied, config)| {
            applied.then_some(config).ok_or_else(|| {
                stale_operation_error(
                    "External command preferences changed; refresh before retrying",
                )
            })
        })?;
    propagate_conflict_preferences(&persisted);
    Ok(persisted)
}

fn reconcile_native_prompt_command_reconfirmation(
    config: &mut ExternalSourcesConfig,
    command_name: &str,
    native_candidate_ids: &[String],
) -> bool {
    let previous_choices = config.conflict_choices.clone();
    let previous_candidates = config.conflicted_candidate_ids.clone();
    let native_group_fingerprint =
        native_prompt_command_group_fingerprint(native_candidate_ids.iter().map(String::as_str));
    for native_candidate_id in native_candidate_ids {
        config.conflicted_candidate_ids.remove(native_candidate_id);
    }
    config.conflict_choices.retain(|key, _| {
        !native_prompt_command_conflict_key_parts(key).is_some_and(
            |(candidate, group_fingerprint)| {
                candidate.eq_ignore_ascii_case(command_name)
                    && group_fingerprint == &native_group_fingerprint[..24]
            },
        )
    });
    config.conflict_choices != previous_choices
        || config.conflicted_candidate_ids != previous_candidates
}

fn native_prompt_command_conflict_key_command(key: &str) -> Option<&str> {
    native_prompt_command_conflict_key_parts(key).map(|(command_name, _)| command_name)
}

fn native_prompt_command_conflict_key_parts(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix("native:prompt_command:")?;
    let (_execution_domain_fingerprint, rest) = rest.split_once(':')?;
    let (command_name_length, encoded_command) = rest.split_once(':')?;
    let command_name_length = command_name_length.parse::<usize>().ok()?;
    let command_name = encoded_command.get(..command_name_length)?;
    let suffix = encoded_command
        .get(command_name_length..)?
        .strip_prefix(':')?;
    let (native_group_fingerprint, participant_fingerprint) = suffix.split_once(':')?;
    if command_name.is_empty()
        || native_group_fingerprint.is_empty()
        || participant_fingerprint.is_empty()
    {
        return None;
    }
    Some((command_name, native_group_fingerprint))
}

fn native_prompt_command_expansion_guard_matches(
    config: &ExternalSourcesConfig,
    conflict_key: &str,
    candidate_id: &str,
    expected_preference_revision: u64,
) -> bool {
    conflict_key.starts_with("native:prompt_command:")
        && config.preference_revision == expected_preference_revision
        && config
            .conflict_choices
            .get(conflict_key)
            .map(String::as_str)
            == Some(candidate_id)
}

fn validate_native_prompt_command_expansion_guard(
    current_conflicts: &[&NativePromptCommandConflictProjection],
    preferences: &ExternalSourcesConfig,
    expected_candidate_id: Option<&str>,
    expected_conflict_key: Option<&str>,
    expected_preference_revision: Option<u64>,
) -> Result<(), String> {
    if current_conflicts.is_empty() {
        return if expected_conflict_key.is_none() && expected_preference_revision.is_none() {
            Ok(())
        } else {
            Err(invalid_operation_error(
                "A native command conflict guard was provided without a current conflict",
            ))
        };
    }
    let (Some(candidate_id), Some(conflict_key), Some(expected_revision)) = (
        expected_candidate_id,
        expected_conflict_key,
        expected_preference_revision,
    ) else {
        return Err(stale_operation_error(
            "The native and external command choice must be provided before expansion",
        ));
    };
    let authorized = current_conflicts.iter().any(|projected| {
        projected.external_candidate_id == candidate_id
            && projected.conflict_key == conflict_key
            && native_prompt_command_expansion_guard_matches(
                preferences,
                conflict_key,
                candidate_id,
                expected_revision,
            )
    });
    authorized.then_some(()).ok_or_else(|| {
        stale_operation_error("The native and external command choice changed before expansion")
    })
}

pub async fn set_external_prompt_command_conflict_choice(
    workspace_id: Option<&str>,
    conflict_key: &str,
    candidate_id: &str,
    expected_preference_revision: u64,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    validate_conflict_preference(conflict_key, candidate_id)?;
    service_for(workspace_id)
        .await?
        .set_conflict_choice(conflict_key, candidate_id, expected_preference_revision)
        .await
}

pub async fn set_external_tool_target_decision(
    workspace_id: Option<&str>,
    approval_key: &str,
    decision_key: &str,
    approved: bool,
    expected_preference_revision: u64,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    service_for(workspace_id)
        .await?
        .set_tool_target_decision(
            approval_key,
            decision_key,
            approved,
            expected_preference_revision,
        )
        .await
}

pub async fn set_external_tool_targets_enabled(
    workspace_id: Option<&str>,
    decisions: Vec<(String, String)>,
    enabled: bool,
    expected_catalog_generation: u64,
    expected_preference_revision: u64,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    service_for(workspace_id)
        .await?
        .set_tool_targets_enabled(
            decisions,
            enabled,
            expected_catalog_generation,
            expected_preference_revision,
        )
        .await
}

pub async fn set_external_tool_conflict_choice(
    workspace_id: Option<&str>,
    conflict_key: &str,
    candidate_id: &str,
    expected_preference_revision: u64,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    service_for(workspace_id)
        .await?
        .set_tool_conflict_choice(conflict_key, candidate_id, expected_preference_revision)
        .await
}

pub async fn set_external_mcp_server_decision(
    workspace_id: Option<&str>,
    candidate_id: &str,
    decision_key: &str,
    approved: bool,
    expected_mcp_generation: u64,
    expected_preference_revision: u64,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    service_for(workspace_id)
        .await?
        .set_mcp_server_decision(
            candidate_id,
            decision_key,
            approved,
            expected_mcp_generation,
            expected_preference_revision,
        )
        .await
}

pub async fn set_external_mcp_servers_enabled(
    workspace_id: Option<&str>,
    decisions: Vec<(String, String)>,
    enabled: bool,
    expected_mcp_generation: u64,
    expected_preference_revision: u64,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    service_for(workspace_id)
        .await?
        .set_mcp_servers_enabled(
            decisions,
            enabled,
            expected_mcp_generation,
            expected_preference_revision,
        )
        .await
}

pub async fn choose_external_mcp_conflict(
    workspace_id: Option<&str>,
    conflict_key: &str,
    candidate_id: &str,
    approve_external: bool,
    expected_mcp_generation: u64,
    expected_preference_revision: u64,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    service_for(workspace_id)
        .await?
        .choose_mcp_conflict(
            conflict_key,
            candidate_id,
            approve_external,
            expected_mcp_generation,
            expected_preference_revision,
        )
        .await
}

pub async fn set_external_subagent_activation(
    workspace_id: Option<&str>,
    candidate_id: &str,
    approved: bool,
    expected_subagent_generation: u64,
    expected_preference_revision: u64,
    decision_key: &str,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    service_for(workspace_id)
        .await?
        .set_subagent_activation(
            candidate_id,
            approved,
            expected_subagent_generation,
            expected_preference_revision,
            decision_key,
        )
        .await
}

pub async fn set_external_subagents_enabled(
    workspace_id: Option<&str>,
    decisions: Vec<(String, String)>,
    enabled: bool,
    expected_subagent_generation: u64,
    expected_preference_revision: u64,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    service_for(workspace_id)
        .await?
        .set_subagents_enabled(
            decisions,
            enabled,
            expected_subagent_generation,
            expected_preference_revision,
        )
        .await
}

pub async fn set_external_subagent_model_binding(
    workspace_id: Option<&str>,
    binding_key: &str,
    target: Option<ExternalSubagentModelBindingTarget>,
    expected_subagent_generation: u64,
    expected_preference_revision: u64,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    service_for(workspace_id)
        .await?
        .set_subagent_model_binding(
            binding_key,
            target,
            expected_subagent_generation,
            expected_preference_revision,
        )
        .await
}

pub async fn choose_external_subagent_conflict(
    workspace_id: Option<&str>,
    conflict_key: &str,
    candidate_id: &str,
    approve_external: bool,
    expected_subagent_generation: u64,
    expected_preference_revision: u64,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    service_for(workspace_id)
        .await?
        .choose_subagent_conflict(
            conflict_key,
            candidate_id,
            approve_external,
            expected_subagent_generation,
            expected_preference_revision,
        )
        .await
}

pub async fn external_source_snapshot(
    workspace_id: Option<&str>,
    force_refresh: bool,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    let service = service_for(workspace_id).await?;
    if force_refresh {
        service.refresh_with_runtime_invalidation().await
    } else {
        service.ensure_background_refresh();
        Ok(service.snapshot())
    }
}

pub async fn workspace_reference_snapshot(
    workspace_id: &str,
    native_related_paths: &[crate::service::workspace::RelatedPath],
    force_refresh: bool,
) -> Result<WorkspaceReferenceSnapshot, String> {
    let service = service_for(Some(workspace_id)).await?;
    sync_service_preferences(&service).await?;
    if force_refresh {
        service.refresh_workspace_references(true).await?;
    } else {
        service.ensure_workspace_reference_refresh().await?;
    }
    let active_ecosystems = ecosystems_with_active_capability(
        &service.snapshot().integration_policy,
        EXTERNAL_CAPABILITY_REFERENCE,
    );
    let external = lock_workspace_reference_coordinator(&service.control_plane).snapshot();
    Ok(compose_workspace_reference_snapshot(
        native_related_paths,
        external,
        &active_ecosystems,
    ))
}

fn compose_workspace_reference_snapshot(
    native_related_paths: &[crate::service::workspace::RelatedPath],
    external: openbitfun_external_sources::ExternalWorkspaceReferenceCoordinatorSnapshot,
    active_ecosystems: &BTreeSet<EcosystemId>,
) -> WorkspaceReferenceSnapshot {
    let external_sources = external
        .sources
        .iter()
        .filter(|source| active_ecosystems.contains(&source.record.ecosystem_id))
        .map(|source| (source.record.key.clone(), &source.record))
        .collect::<BTreeMap<_, _>>();
    let mut references = native_related_paths
        .iter()
        .map(|related_path| WorkspaceReferenceCatalogEntry {
            stable_key: native_workspace_reference_key(related_path),
            alias: None,
            path: PathBuf::from(&related_path.path),
            description: related_path.description.clone(),
            hidden: false,
            origin: WorkspaceReferenceOrigin::Native,
            ecosystem_id: None,
            source_display_name: None,
            source_scope: None,
        })
        .collect::<Vec<_>>();
    references.extend(external.references.into_iter().filter_map(|reference| {
        let source = external_sources.get(&reference.source)?;
        Some(WorkspaceReferenceCatalogEntry {
            stable_key: reference.stable_key(),
            alias: Some(reference.alias),
            path: reference.path,
            description: reference.description,
            hidden: reference.hidden,
            origin: WorkspaceReferenceOrigin::External,
            ecosystem_id: Some(source.ecosystem_id.clone()),
            source_display_name: Some(source.display_name.clone()),
            source_scope: Some(source.scope),
        })
    }));
    WorkspaceReferenceSnapshot {
        generation: external.generation,
        discovery_pending: external.discovery_pending,
        references,
        diagnostics: external.diagnostics,
    }
}

fn native_workspace_reference_key(related_path: &crate::service::workspace::RelatedPath) -> String {
    let mut hasher = Sha256::new();
    hasher.update(related_path.path.as_bytes());
    hasher.update([0]);
    hasher.update(related_path.description.as_deref().unwrap_or("").as_bytes());
    format!("native:{}", hex::encode(hasher.finalize()))
}

/// Resolves an opaque source identity to its host-local location for a host
/// action. The raw path must not be serialized into the public snapshot.
pub async fn external_source_location_for_host_action(
    workspace_id: Option<&str>,
    stable_key: &str,
) -> Result<PathBuf, String> {
    service_for(workspace_id).await?.source_location(stable_key)
}

pub async fn get_external_source_control_snapshot(
    workspace_id: Option<&str>,
    force_refresh: bool,
    host_capabilities: ExternalSourceHostCapabilities,
) -> ExternalSourceOperationResult<ExternalSourceSurfaceSnapshotV1> {
    use openbitfun_product_domains::external_source_control::ExternalSourceOperationStage;

    let service = if host_capabilities.can_execute_external_assets {
        service_for(workspace_id).await
    } else {
        read_only_service_for(workspace_id).await
    }
    .map_err(|error| {
        sanitize_external_source_operation_error(error)
            .with_stage(ExternalSourceOperationStage::ProjectResponse)
    })?;
    if force_refresh {
        service
            .refresh_with_runtime_invalidation()
            .await
            .map_err(|error| {
                sanitize_external_source_operation_error(error)
                    .with_stage(ExternalSourceOperationStage::Discover)
            })?;
    } else {
        service.ensure_background_refresh();
    }
    Ok(service.surface_snapshot(host_capabilities))
}

pub async fn apply_external_source_control_action(
    workspace_id: Option<&str>,
    request: ExternalSourceControlRequestV1,
) -> ExternalSourceOperationResult<ExternalSourceSurfaceSnapshotV1> {
    use openbitfun_product_domains::external_source_control::ExternalSourceOperationStage;

    let operation_id = request.operation_id.clone();
    let service = service_for(workspace_id).await.map_err(|error| {
        typed_control_operation_error(
            error,
            &operation_id,
            ExternalSourceOperationStage::ValidateRequest,
        )
    })?;
    service.apply_control_action(request).await
}

/// Returns a static, sanitized projection for Hosts that may inspect external
/// configuration but must never load external code or alter runtime routes.
pub async fn external_source_read_only_snapshot(
    workspace_id: Option<&str>,
    force_refresh: bool,
) -> Result<ExternalSourcePublicSnapshot, String> {
    let service = read_only_service_for(workspace_id).await?;
    let snapshot = if force_refresh {
        service.refresh().await?
    } else {
        service.ensure_background_refresh();
        service.snapshot()
    };
    let mut public = ExternalSourcePublicSnapshot::from(snapshot);
    public.host_capabilities = ExternalSourceHostCapabilities::read_only_projection();
    Ok(public)
}

pub async fn external_source_discovery_snapshot(
    workspace_id: Option<&str>,
    force_refresh: bool,
    host_capabilities: ExternalSourceHostCapabilities,
) -> Result<ExternalSourceDiscoverySnapshotV1, String> {
    let service =
        service_for_profile(workspace_id, ExternalSourceServiceProfile::DiscoveryCatalog).await?;
    if force_refresh {
        service.refresh().await?;
    } else {
        if service.automatic_discovery_allowed().await? {
            service.ensure_background_refresh();
        }
    }
    let preferences = read_external_sources_config().await?;
    let automatic_discovery = preferences
        .integration_policy
        .known()
        .is_some_and(|document| {
            automatic_discovery_enabled(
                document,
                workspace_policy_key(service.workspace_id.as_deref()).as_deref(),
            )
        });
    let has_scanned = service.initial_refresh_completed.load(Ordering::Acquire);
    let discovery_policy = service.discovery_policy(&preferences)?;
    let discoverable_capabilities = discovery_policy
        .effective
        .ecosystems
        .iter()
        .map(|(id, ecosystem)| {
            (
                id.clone(),
                ecosystem
                    .capabilities
                    .iter()
                    .filter_map(|(capability, _)| {
                        integration_capability_is_discoverable(
                            &discovery_policy,
                            id.as_str(),
                            capability.as_str(),
                        )
                        .then_some(capability.clone())
                    })
                    .collect(),
            )
        })
        .collect();
    // Read the result after the awaited preference read, so a just-completed
    // background scan cannot be reported as complete with an earlier catalog.
    let mut catalog = ExternalSourcePublicSnapshot::from(service.snapshot());
    catalog.host_capabilities = ExternalSourceHostCapabilities::read_only_projection();
    catalog.integration_policy =
        integration_policy_snapshot(&preferences, service.workspace_id.as_deref())?;
    catalog.preference_revision = preferences.preference_revision;
    catalog.discovery_pending = service.background_refresh_scheduled.load(Ordering::Acquire)
        || service.refresh_gate.try_lock().is_err()
        || if has_scanned {
            catalog.discovery_pending
        } else {
            automatic_discovery
        };
    Ok(ExternalSourceDiscoverySnapshotV1 {
        schema_version: 1,
        automatic_discovery,
        can_change_automatic_discovery: host_capabilities.can_mutate_policy
            && preferences.integration_policy.known().is_some(),
        has_scanned,
        discoverable_capabilities,
        preference_revision: preferences.preference_revision,
        catalog,
    })
}

pub async fn update_external_integration_policy(
    workspace_id: Option<&str>,
    mutation: ExternalIntegrationPolicyMutation,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    let expected_revision = mutation.expected_preference_revision;
    let (scope, operation, ecosystem, capability) = integration_policy_log_context(&mutation);
    let result = if matches!(
        mutation.change,
        ExternalIntegrationPolicyOperation::SetAutomaticDiscovery { .. }
    ) {
        async {
            persist_integration_policy_mutation(workspace_id, mutation).await?;
            let service =
                service_for_profile(workspace_id, ExternalSourceServiceProfile::DiscoveryCatalog)
                    .await?;
            service.ensure_initial_refresh().await
        }
        .await
    } else {
        match service_for(workspace_id).await {
            Ok(service) => service.update_integration_policy(mutation).await,
            Err(error) => Err(error),
        }
    };
    match &result {
        Ok(snapshot) => log::info!(
            "External integration policy mutation outcome=success scope={} operation={} ecosystem={} capability={} revision={} changed={}",
            scope,
            operation,
            ecosystem,
            capability,
            snapshot.preference_revision,
            snapshot.preference_revision != expected_revision,
        ),
        Err(error) => log::warn!(
            "External integration policy mutation outcome=failure scope={} operation={} ecosystem={} capability={} expected_revision={} error_code={}",
            scope,
            operation,
            ecosystem,
            capability,
            expected_revision,
            external_integration_error_code(error),
        ),
    }
    result
}

fn integration_policy_log_context(
    mutation: &ExternalIntegrationPolicyMutation,
) -> (&'static str, &'static str, String, String) {
    let scope = match mutation.scope {
        ExternalIntegrationPolicyScope::User => "user",
        ExternalIntegrationPolicyScope::Workspace => "workspace",
        _ => "unknown",
    };
    let (operation, ecosystem, capability) = match &mutation.change {
        ExternalIntegrationPolicyOperation::SetAutomaticDiscovery { .. } => (
            "set_automatic_discovery",
            "all".to_string(),
            "all".to_string(),
        ),
        ExternalIntegrationPolicyOperation::SetEnabled { .. } => {
            ("set_enabled", "all".to_string(), "all".to_string())
        }
        ExternalIntegrationPolicyOperation::SetEcosystemMode { ecosystem_id, .. } => (
            "set_ecosystem_mode",
            safe_external_log_token(ecosystem_id.as_str()),
            "all".to_string(),
        ),
        ExternalIntegrationPolicyOperation::SetCapabilityAccess {
            ecosystem_id,
            capability_id,
            ..
        } => (
            "set_capability_access",
            safe_external_log_token(ecosystem_id.as_str()),
            safe_external_log_token(capability_id.as_str()),
        ),
        ExternalIntegrationPolicyOperation::ResetWorkspace => {
            ("reset_workspace", "all".to_string(), "all".to_string())
        }
        ExternalIntegrationPolicyOperation::ResetIncompatiblePolicy => (
            "reset_incompatible_policy",
            "all".to_string(),
            "all".to_string(),
        ),
        _ => ("unknown", "unknown".to_string(), "unknown".to_string()),
    };
    (scope, operation, ecosystem, capability)
}

fn safe_external_log_token(value: &str) -> String {
    value
        .chars()
        .take(64)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn external_log_scope<T>(workspace_root: Option<T>) -> &'static str {
    if workspace_root.is_some() {
        "workspace"
    } else {
        "user-global"
    }
}

fn external_log_error_category(error: &str) -> String {
    ExternalSourceOperationError::decode(error)
        .map(|typed| typed.code.as_str().to_string())
        .unwrap_or_else(|| "internal".to_string())
}

/// Converts legacy internal failures at the product boundary without deriving
/// control flow from prose. Callers may pass an exactly encoded shared error;
/// every other failure becomes a sanitized internal error with a correlation
/// id, while the local log retains only a bounded category token.
pub fn sanitize_external_source_operation_error(error: String) -> ExternalSourceOperationError {
    if let Some(typed) = ExternalSourceOperationError::decode(&error) {
        return typed.with_default_recovery_actions();
    }
    static NEXT_CORRELATION_ID: AtomicU64 = AtomicU64::new(1);
    let correlation_id = format!(
        "external-source-{}-{}",
        epoch_seconds(),
        NEXT_CORRELATION_ID.fetch_add(1, Ordering::Relaxed)
    );
    log::error!(
        "External source operation failed correlation_id={} category={}",
        correlation_id,
        external_log_error_category(&error),
    );
    ExternalSourceOperationError::new(
        ExternalSourceOperationErrorCode::Internal,
        "External source operation failed. Retry, then use the reference id if the problem continues.",
        true,
    )
    .with_correlation_id(correlation_id)
    .with_default_recovery_actions()
}

fn typed_control_operation_error(
    error: String,
    operation_id: &str,
    stage: openbitfun_product_domains::external_source_control::ExternalSourceOperationStage,
) -> ExternalSourceOperationError {
    let mut typed = sanitize_external_source_operation_error(error);
    if typed.correlation_id.is_none() {
        typed = typed.with_correlation_id(operation_id);
    } else if typed.causation_id.is_none() {
        typed = typed.with_causation_id(operation_id);
    }
    if typed.stage.is_none() {
        typed = typed.with_stage(stage);
    }
    typed
}

fn encoded_operation_error(
    code: ExternalSourceOperationErrorCode,
    detail: impl Into<String>,
    retryable: bool,
) -> String {
    ExternalSourceOperationError::new(code, detail, retryable).encode()
}

fn stale_operation_error(detail: impl Into<String>) -> String {
    encoded_operation_error(
        ExternalSourceOperationErrorCode::StaleRevision,
        detail,
        true,
    )
}

fn missing_candidate_error(detail: impl Into<String>) -> String {
    encoded_operation_error(ExternalSourceOperationErrorCode::NotFound, detail, false)
}

fn policy_limited_error(detail: impl Into<String>) -> String {
    encoded_operation_error(
        ExternalSourceOperationErrorCode::PolicyLimited,
        detail,
        false,
    )
}

fn conflict_operation_error(detail: impl Into<String>) -> String {
    encoded_operation_error(ExternalSourceOperationErrorCode::Conflict, detail, true)
}

fn unavailable_operation_error(detail: impl Into<String>) -> String {
    encoded_operation_error(ExternalSourceOperationErrorCode::Unavailable, detail, true)
}

fn invalid_operation_error(detail: impl Into<String>) -> String {
    encoded_operation_error(
        ExternalSourceOperationErrorCode::InvalidRequest,
        detail,
        false,
    )
}

fn incompatible_policy_error(detail: impl Into<String>) -> String {
    encoded_operation_error(
        ExternalSourceOperationErrorCode::PolicyIncompatible,
        detail,
        false,
    )
}

pub(crate) fn external_integration_error_code(error: &str) -> String {
    ExternalSourceOperationError::decode(error)
        .map(|error| error.code.as_str().to_string())
        .unwrap_or_else(|| "internal".to_string())
}

/// Ensure the workspace-scoped static source snapshot has been published once.
/// This is the shared discovery gate for selectors and execution routing; it
/// does not start or recover any executable extension runtime.
pub async fn ensure_external_source_workspace_snapshot(
    workspace_id: Option<&str>,
) -> Result<(), String> {
    ensure_initial_external_source_workspace_service(workspace_id)
        .await
        .map(|_| ())
}

async fn ensure_initial_external_source_workspace_service(
    workspace_id: Option<&str>,
) -> Result<Arc<WorkspaceExternalSourceService>, String> {
    let service = service_for(workspace_id).await?;
    service.ensure_initial_refresh().await?;
    Ok(service)
}

/// Keep the external-source runtime aligned with an actively assembled product
/// tool catalog. A newly created service performs one synchronous refresh so an
/// idle-retired workspace can restore approved routes before the catalog is
/// exposed to the model. Existing services are only touched; file watchers and
/// explicit refreshes remain responsible for later source changes.
pub(crate) async fn ensure_external_source_workspace_runtime(workspace_id: Option<&str>) {
    let service = match ensure_initial_external_source_workspace_service(workspace_id).await {
        Ok(service) => service,
        Err(error) => {
            log::warn!(
                "Could not initialize external source workspace runtime scope={} error_category={}",
                external_log_scope(workspace_id),
                external_log_error_category(&error),
            );
            return;
        }
    };
    if external_tool_workspace_requires_recovery(workspace_id).await {
        if let Err(error) = service.refresh_worker_loss_once().await {
            log::warn!(
                "Could not recover external source tool runtime scope={} error_category={}",
                external_log_scope(workspace_id),
                external_log_error_category(&error),
            );
        }
    }
}

pub async fn set_external_source_enabled(
    workspace_id: Option<&str>,
    source_key: &str,
    enabled: bool,
    expected_preference_revision: u64,
) -> Result<ExternalSourceCatalogSnapshot, String> {
    service_for(workspace_id)
        .await?
        .set_source_enabled(source_key, enabled, expected_preference_revision)
        .await
}

pub async fn expand_external_prompt_command(
    workspace_id: Option<&str>,
    name: &str,
    arguments: &str,
    native_commands: Vec<NativePromptCommandDescriptor>,
    expected_candidate_id: Option<&str>,
    expected_content_version: Option<&str>,
    expected_native_conflict_key: Option<&str>,
    expected_preference_revision: Option<u64>,
    shell_review_decision: Option<&PromptCommandShellReviewDecision>,
) -> Result<PromptCommandInvocationOutcome, String> {
    service_for(workspace_id)
        .await?
        .expand_command(
            name,
            arguments,
            &native_commands,
            expected_candidate_id,
            expected_content_version,
            expected_native_conflict_key,
            expected_preference_revision,
            shell_review_decision,
        )
        .await
}

pub async fn subscribe_external_source_updates(
    workspace_id: Option<&str>,
) -> Result<ExternalSourceSubscription, String> {
    let service = service_for(workspace_id).await?;
    let receiver = service.updates.subscribe();
    service.ensure_background_refresh();
    Ok(ExternalSourceSubscription {
        _service: service,
        receiver,
    })
}

pub struct ExternalSourceSubscription {
    _service: Arc<WorkspaceExternalSourceService>,
    receiver: broadcast::Receiver<ExternalSourceCatalogSnapshot>,
}

impl ExternalSourceSubscription {
    pub async fn recv(
        &mut self,
    ) -> Result<ExternalSourceCatalogSnapshot, broadcast::error::RecvError> {
        self.receiver.recv().await
    }

    pub fn try_recv(
        &mut self,
    ) -> Result<ExternalSourceCatalogSnapshot, broadcast::error::TryRecvError> {
        self.receiver.try_recv()
    }
}

#[cfg(test)]
mod opencode_local_source_order_tests;

#[cfg(test)]
mod tests;
