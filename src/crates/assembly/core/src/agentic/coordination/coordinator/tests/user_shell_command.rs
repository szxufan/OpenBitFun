use super::super::test_support::{test_coordinator, test_coordinator_with_registry};
use super::super::ConversationCoordinator;
use crate::agentic::core::{MessageContent, SessionConfig};
use crate::agentic::events::AgenticEvent;
use crate::agentic::session::SessionManager;
use crate::agentic::tools::framework::{
    PermissionIntent, Tool, ToolResult, ToolUseContext, ValidationResult,
};
use crate::agentic::tools::registry::ToolRegistry;
use crate::runtime_ownership::CoreRuntimeOwnership;
use crate::service::session::{DialogTurnKind, TurnStatus};
use openbitfun_agent_runtime::permission::PermissionRequestManager;
use openbitfun_runtime_ports::{
    AgentUserShellCommandPort, AgentUserShellCommandRequest, PortErrorKind,
};
use openbitfun_runtime_services::test_support::FakeRuntimePort;
use openbitfun_services_core::permission_store::ProjectPermissionSqliteStore;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::Notify;

// These tests settle only after the real filesystem and SQLite persistence path completes.
// Keep the wait state-based, but allow for loaded hosted Windows runners.
const USER_SHELL_TURN_SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
struct TestExecCommandTool {
    validation_started: Option<Arc<Notify>>,
    release_validation: Option<Arc<Notify>>,
    call_count: Option<Arc<AtomicUsize>>,
}

#[async_trait::async_trait]
impl Tool for TestExecCommandTool {
    fn name(&self) -> &str {
        "ExecCommand"
    }

    async fn description(&self) -> crate::util::errors::OpenBitFunResult<String> {
        Ok("test user shell command".to_string())
    }

    fn short_description(&self) -> String {
        "test user shell command".to_string()
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["cmd"],
            "properties": {
                "cmd": { "type": "string" },
                "tty": { "type": "boolean" }
            },
            "additionalProperties": false
        })
    }

    fn is_readonly(&self) -> bool {
        false
    }

    fn permission_intents(
        &self,
        input: &serde_json::Value,
        _context: &ToolUseContext,
    ) -> crate::util::errors::OpenBitFunResult<Vec<PermissionIntent>> {
        Ok(vec![PermissionIntent::new(
            "bash",
            vec![input["cmd"].as_str().unwrap_or_default().to_string()],
        )])
    }

    async fn validate_input(
        &self,
        _input: &serde_json::Value,
        _context: Option<&ToolUseContext>,
    ) -> ValidationResult {
        if let Some(started) = &self.validation_started {
            started.notify_one();
        }
        if let Some(release) = &self.release_validation {
            release.notified().await;
        }
        ValidationResult {
            result: true,
            message: None,
            error_code: None,
            meta: None,
        }
    }

    async fn call_impl(
        &self,
        input: &serde_json::Value,
        _context: &ToolUseContext,
    ) -> crate::util::errors::OpenBitFunResult<Vec<ToolResult>> {
        if let Some(call_count) = &self.call_count {
            call_count.fetch_add(1, Ordering::SeqCst);
        }
        let command = input["cmd"].as_str().unwrap_or_default();
        let exit_code = if command == "exit 7" { 7 } else { 0 };
        Ok(vec![ToolResult::Result {
            data: serde_json::json!({
                "exit_code": exit_code,
                "output": command,
            }),
            result_for_assistant: Some(command.to_string()),
            image_attachments: None,
        }])
    }
}

fn test_persistent_user_shell_coordinator_with_tool(
    tool: Arc<dyn Tool>,
) -> (ConversationCoordinator, Arc<SessionManager>) {
    let ownership_root = std::env::temp_dir().join(format!(
        "openbitfun-runtime-ownership-test-{}",
        uuid::Uuid::new_v4()
    ));
    let mut registry = ToolRegistry::new();
    registry.register_tool(tool);
    let permission_store = Arc::new(ProjectPermissionSqliteStore::new(
        ownership_root.join("permissions"),
    ));
    let permission_request_manager = Arc::new(
        PermissionRequestManager::new(
            permission_store.clone(),
            permission_store.clone(),
            Arc::new(FakeRuntimePort::new(
                openbitfun_runtime_ports::RuntimeServiceCapability::Clock,
            )),
        )
        .with_grant_store(permission_store),
    );
    test_coordinator_with_registry(
        100,
        true,
        Arc::new(CoreRuntimeOwnership::embedded_with_facts(
            ownership_root,
            "openbitfun".to_string(),
            "test",
        )),
        registry,
        Some(permission_request_manager),
    )
}

fn test_persistent_user_shell_coordinator() -> (ConversationCoordinator, Arc<SessionManager>) {
    test_persistent_user_shell_coordinator_with_tool(Arc::new(TestExecCommandTool::default()))
}

#[tokio::test]
async fn user_shell_command_rejects_blank_and_nul_input_before_admission() {
    let (coordinator, _) = test_coordinator();

    for command in ["   ", "printf 'bad\0input'"] {
        let error = AgentUserShellCommandPort::run_user_shell_command(
            &coordinator,
            AgentUserShellCommandRequest {
                session_id: "missing-session".to_string(),
                turn_id: "turn-shell".to_string(),
                command: command.to_string(),
            },
        )
        .await
        .expect_err("invalid commands must fail before session lookup");

        assert_eq!(error.kind, PortErrorKind::InvalidRequest);
    }
}

#[tokio::test]
async fn user_shell_command_persists_a_standard_exec_command_tool_turn() {
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, session_manager) = test_persistent_user_shell_coordinator();
    let session = session_manager
        .create_session(
            "Shell turn".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("create session");

    let accepted = AgentUserShellCommandPort::run_user_shell_command(
        &coordinator,
        AgentUserShellCommandRequest {
            session_id: session.session_id.clone(),
            turn_id: "turn-shell".to_string(),
            command: "git status --short".to_string(),
        },
    )
    .await
    .expect("admit shell turn");
    coordinator
        .wait_for_turn_settlement(
            &accepted.session_id,
            &accepted.turn_id,
            USER_SHELL_TURN_SETTLEMENT_TIMEOUT,
        )
        .await
        .expect("shell turn settles");

    let turns = session_manager
        .persistence_manager()
        .load_session_turns(workspace.path(), &session.session_id)
        .await
        .expect("load turns");
    let turn = turns.last().expect("shell turn");
    let events = coordinator.event_queue.dequeue_batch(100).await;
    let started_binding = events.iter().find_map(|envelope| match &envelope.event {
        AgenticEvent::DialogTurnStarted {
            turn_id,
            session_binding,
            ..
        } if turn_id == "turn-shell" => Some(session_binding.clone()),
        _ => None,
    });
    // A surface that has not loaded the session attributes the Turn to
    // its owning workspace from this binding, not to its current one.
    assert_eq!(
        started_binding
            .expect("turn started event")
            .and_then(|binding| binding.workspace_path),
        session.config.workspace_path
    );
    assert!(!events.iter().any(|envelope| matches!(
        &envelope.event,
        AgenticEvent::DialogTurnFailed { turn_id, .. } if turn_id == "turn-shell"
    )));
    assert_eq!(turn.kind, DialogTurnKind::UserDialog);
    assert_eq!(turn.user_message.content, "!git status --short");
    assert_eq!(
        turn.status,
        TurnStatus::Completed,
        "shell turn failed: error={:?}, events={events:?}",
        turn.error,
    );
    let tool = turn
        .model_rounds
        .first()
        .and_then(|round| round.tool_items.first())
        .expect("ExecCommand tool item");
    assert_eq!(tool.tool_name, "ExecCommand");
    assert_eq!(tool.tool_call.input["cmd"], "git status --short");
    assert!(tool.tool_result.is_some());

    let context = session_manager
        .get_context_messages(&session.session_id)
        .await
        .expect("context messages");
    assert!(context.iter().any(|message| matches!(
        &message.content,
        MessageContent::Mixed { tool_calls, .. }
            if tool_calls.iter().any(|call| call.tool_name == "ExecCommand")
    )));
    assert!(context
        .iter()
        .any(|message| matches!(message.content, MessageContent::ToolResult { .. })));
}

#[tokio::test]
async fn user_shell_command_auto_approves_ask_but_preserves_project_denies() {
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let permission_path = workspace
        .path()
        .join(".openbitfun")
        .join("config")
        .join("tool_permissions.json");
    tokio::fs::create_dir_all(permission_path.parent().expect("permission parent"))
        .await
        .expect("create permission directory");
    tokio::fs::write(
        &permission_path,
        r#"{"rules":[{"action":"bash","resource":"git reset --hard","effect":"deny"}]}"#,
    )
    .await
    .expect("write project permission rule");

    let (coordinator, session_manager) = test_persistent_user_shell_coordinator();
    let session = session_manager
        .create_session(
            "Denied shell turn".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("create session");
    let accepted = AgentUserShellCommandPort::run_user_shell_command(
        &coordinator,
        AgentUserShellCommandRequest {
            session_id: session.session_id.clone(),
            turn_id: "turn-shell-denied".to_string(),
            command: "git reset --hard".to_string(),
        },
    )
    .await
    .expect("deny is represented as a settled tool result");
    coordinator
        .wait_for_turn_settlement(
            &accepted.session_id,
            &accepted.turn_id,
            USER_SHELL_TURN_SETTLEMENT_TIMEOUT,
        )
        .await
        .expect("denied shell turn settles");

    let turns = session_manager
        .persistence_manager()
        .load_session_turns(workspace.path(), &session.session_id)
        .await
        .expect("load turns");
    let tool_result = turns
        .last()
        .and_then(|turn| turn.model_rounds.first())
        .and_then(|round| round.tool_items.first())
        .and_then(|tool| tool.tool_result.as_ref())
        .expect("permission denial tool result");
    assert_eq!(tool_result.result["category"], "permission_denied");
    let events = coordinator.event_queue.dequeue_batch(100).await;
    assert!(events.iter().any(|envelope| matches!(
        &envelope.event,
        AgenticEvent::DialogTurnCompleted {
            turn_id,
            success: Some(false),
            finish_reason: Some(reason),
            ..
        } if turn_id == "turn-shell-denied" && reason == "tool_error"
    )));
}

#[tokio::test]
async fn user_shell_command_reports_a_nonzero_exit_as_a_tool_error() {
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, session_manager) = test_persistent_user_shell_coordinator();
    let session = session_manager
        .create_session(
            "Failed shell turn".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("create session");
    let accepted = AgentUserShellCommandPort::run_user_shell_command(
        &coordinator,
        AgentUserShellCommandRequest {
            session_id: session.session_id,
            turn_id: "turn-shell-nonzero".to_string(),
            command: "exit 7".to_string(),
        },
    )
    .await
    .expect("admit shell turn");
    coordinator
        .wait_for_turn_settlement(
            &accepted.session_id,
            &accepted.turn_id,
            USER_SHELL_TURN_SETTLEMENT_TIMEOUT,
        )
        .await
        .expect("failed shell turn settles");

    let events = coordinator.event_queue.dequeue_batch(100).await;
    assert!(events.iter().any(|envelope| matches!(
        &envelope.event,
        AgenticEvent::DialogTurnCompleted {
            turn_id,
            success: Some(false),
            finish_reason: Some(reason),
            ..
        } if turn_id == "turn-shell-nonzero" && reason == "tool_error"
    )));
}

#[tokio::test]
async fn user_shell_command_cancelled_during_validation_never_executes() {
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let validation_started = Arc::new(Notify::new());
    let release_validation = Arc::new(Notify::new());
    let call_count = Arc::new(AtomicUsize::new(0));
    let (coordinator, session_manager) =
        test_persistent_user_shell_coordinator_with_tool(Arc::new(TestExecCommandTool {
            validation_started: Some(validation_started.clone()),
            release_validation: Some(release_validation.clone()),
            call_count: Some(call_count.clone()),
        }));
    let coordinator = Arc::new(coordinator);
    let session = session_manager
        .create_session(
            "Cancelled shell turn".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("create session");
    let turn_id = "turn-shell-cancel-preflight".to_string();
    let accepted = AgentUserShellCommandPort::run_user_shell_command(
        coordinator.as_ref(),
        AgentUserShellCommandRequest {
            session_id: session.session_id.clone(),
            turn_id: turn_id.clone(),
            command: "touch must-not-run".to_string(),
        },
    )
    .await
    .expect("admit shell turn");
    tokio::time::timeout(Duration::from_secs(1), validation_started.notified())
        .await
        .expect("tool validation starts");

    let coordinator_for_cancel = coordinator.clone();
    let session_id_for_cancel = session.session_id.clone();
    let turn_id_for_cancel = turn_id.clone();
    let cancel_task = tokio::spawn(async move {
        openbitfun_runtime_ports::AgentTurnCancellationPort::cancel_turn(
            coordinator_for_cancel.as_ref(),
            openbitfun_runtime_ports::AgentTurnCancellationRequest {
                session_id: session_id_for_cancel,
                turn_id: Some(turn_id_for_cancel),
                source: None,
                requester_session_id: None,
                reason: Some("test cancellation".to_string()),
                wait_timeout_ms: Some(1500),
                cancel_descendants: true,
            },
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if coordinator
                .execution_cancel_token_for_dialog_turn(&turn_id)
                .is_some_and(|token| token.is_cancelled())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("turn cancellation is signalled");
    release_validation.notify_one();
    cancel_task
        .await
        .expect("cancel task joins")
        .expect("cancel request succeeds");
    coordinator
        .wait_for_turn_settlement(
            &accepted.session_id,
            &accepted.turn_id,
            USER_SHELL_TURN_SETTLEMENT_TIMEOUT,
        )
        .await
        .expect("cancelled shell turn settles");

    assert_eq!(call_count.load(Ordering::SeqCst), 0);
}
