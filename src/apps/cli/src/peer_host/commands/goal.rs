//! Thread goal HostInvoke handlers for CLI Peer Host.
//!
//! These mirror the Desktop goal commands so a controller can activate, pause,
//! resume, edit, and clear a session goal on this host exactly as it does
//! locally. Reading a goal stays with `session::get_session_thread_goal`.

use std::path::PathBuf;
use std::sync::Arc;

use openbitfun_core::agentic::coordination::{get_global_coordinator, ConversationCoordinator};
use openbitfun_core::agentic::goal_mode::ThreadGoalStatus;
use serde_json::{json, Value};

use crate::peer_host::args::{get_string, optional_string, request_value};
use crate::peer_host::state::PeerHostState;

use super::session::resolved_session_storage_path;

fn coordinator() -> Result<Arc<ConversationCoordinator>, String> {
    get_global_coordinator().ok_or_else(|| "Conversation coordinator is unavailable".to_string())
}

fn required_session_id(request: &Value) -> Result<String, String> {
    let session_id = get_string(request, "sessionId")?.trim().to_string();
    if session_id.is_empty() {
        return Err("session_id is required".to_string());
    }
    Ok(session_id)
}

/// Load the session into the coordinator when it is only persisted.
async fn ensure_session_loaded(
    state: &PeerHostState,
    coordinator: &ConversationCoordinator,
    request: &Value,
    session_id: &str,
    purpose: &str,
) -> Result<(), String> {
    if coordinator
        .get_session_manager()
        .get_session(session_id)
        .is_some()
    {
        return Ok(());
    }
    let storage = resolved_session_storage_path(state, request).await?;
    coordinator
        .restore_session_from_storage_path(&storage, session_id)
        .await
        .map(|_| ())
        .map_err(|error| format!("Failed to restore session before {purpose}: {error}"))
}

/// The goal APIs address a session by its execution workspace.
async fn goal_workspace_path(
    state: &PeerHostState,
    coordinator: &ConversationCoordinator,
    request: &Value,
    session_id: &str,
) -> Result<PathBuf, String> {
    ensure_session_loaded(
        state,
        coordinator,
        request,
        session_id,
        "thread goal access",
    )
    .await?;
    coordinator
        .get_session_manager()
        .get_session(session_id)
        .and_then(|session| session.config.workspace_path.clone())
        .map(PathBuf::from)
        .ok_or_else(|| format!("Session workspace_path is missing: {session_id}"))
}

fn parse_goal_status(status: &str) -> Result<ThreadGoalStatus, String> {
    match status.trim().to_ascii_lowercase().as_str() {
        "active" => Ok(ThreadGoalStatus::Active),
        "paused" => Ok(ThreadGoalStatus::Paused),
        "blocked" => Ok(ThreadGoalStatus::Blocked),
        "usagelimited" | "usage_limited" => Ok(ThreadGoalStatus::UsageLimited),
        "budgetlimited" | "budget_limited" => Ok(ThreadGoalStatus::BudgetLimited),
        "complete" => Ok(ThreadGoalStatus::Complete),
        other => Err(format!("unsupported thread goal status: {other}")),
    }
}

pub(crate) async fn activate_session_goal(
    state: &PeerHostState,
    args: &Value,
) -> Result<Value, String> {
    let request = request_value(args);
    let session_id = required_session_id(request)?;
    let coordinator = coordinator()?;
    ensure_session_loaded(
        state,
        &coordinator,
        request,
        &session_id,
        "activating goal mode",
    )
    .await?;
    let user_hint = optional_string(request, "userHint")
        .map(|hint| hint.trim().to_string())
        .filter(|hint| !hint.is_empty());
    let goal = coordinator
        .activate_session_goal(session_id, user_hint)
        .await
        .map_err(|error| error.to_string())?;
    Ok(json!({ "success": true, "goal": goal }))
}

pub(crate) async fn set_session_thread_goal_status(
    state: &PeerHostState,
    args: &Value,
) -> Result<Value, String> {
    let request = request_value(args);
    let session_id = required_session_id(request)?;
    let status = parse_goal_status(&get_string(request, "status")?)?;
    let coordinator = coordinator()?;
    let workspace = goal_workspace_path(state, &coordinator, request, &session_id).await?;
    let goal = coordinator
        .set_thread_goal_status(&session_id, workspace.as_path(), status)
        .await
        .map_err(|error| error.to_string())?;
    serde_json::to_value(goal).map_err(|error| error.to_string())
}

pub(crate) async fn update_session_thread_goal_objective(
    state: &PeerHostState,
    args: &Value,
) -> Result<Value, String> {
    let request = request_value(args);
    let session_id = required_session_id(request)?;
    let objective = get_string(request, "objective")?.trim().to_string();
    if objective.is_empty() {
        return Err("objective is required".to_string());
    }
    let coordinator = coordinator()?;
    let workspace = goal_workspace_path(state, &coordinator, request, &session_id).await?;
    let goal = coordinator
        .update_thread_goal_objective(&session_id, workspace.as_path(), objective)
        .await
        .map_err(|error| error.to_string())?;
    serde_json::to_value(goal).map_err(|error| error.to_string())
}

pub(crate) async fn clear_session_thread_goal(
    state: &PeerHostState,
    args: &Value,
) -> Result<Value, String> {
    let request = request_value(args);
    let session_id = required_session_id(request)?;
    let coordinator = coordinator()?;
    let workspace = goal_workspace_path(state, &coordinator, request, &session_id).await?;
    coordinator
        .clear_thread_goal(&session_id, workspace.as_path())
        .await
        .map_err(|error| error.to_string())?;
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_status_accepts_the_desktop_spellings() {
        for (wire, expected) in [
            ("active", ThreadGoalStatus::Active),
            ("Paused", ThreadGoalStatus::Paused),
            ("blocked", ThreadGoalStatus::Blocked),
            ("usageLimited", ThreadGoalStatus::UsageLimited),
            ("usage_limited", ThreadGoalStatus::UsageLimited),
            ("budgetLimited", ThreadGoalStatus::BudgetLimited),
            ("complete", ThreadGoalStatus::Complete),
        ] {
            assert_eq!(parse_goal_status(wire).unwrap(), expected, "{wire}");
        }
        assert!(parse_goal_status("archived").is_err());
    }

    #[test]
    fn blank_session_ids_are_rejected_before_any_runtime_access() {
        assert_eq!(
            required_session_id(&json!({ "sessionId": "  " })).unwrap_err(),
            "session_id is required"
        );
    }
}
