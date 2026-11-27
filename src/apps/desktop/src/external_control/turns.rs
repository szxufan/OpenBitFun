use super::{
    eligible,
    http::{Error, Host},
    store::{digest, turn_id, Grant, Receipt},
};
use crate::{
    api::{
        agentic_api::{create_desktop_session, CreateSessionRequest},
        app_state::AppState,
    },
    runtime::{DesktopRuntimeContext, DesktopSessionScopeRequest},
};
use axum::http::StatusCode;
use openbitfun_core::{
    agentic::{
        coordination::{ConversationCoordinator, DialogScheduler},
        core::SessionState,
    },
    service::{
        session::{DialogTurnData, SessionTurnWindowResponse, TurnStatus},
        workspace::WorkspaceInfo,
    },
};
use openbitfun_runtime_ports::{
    AgentDialogTurnRequest, AgentSubmissionSource, AgentTurnCancellationRequest,
    DialogSubmissionPolicy, DialogSubmitOutcome, SessionTurnWindowRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Submit {
    workspace_id: String,
    session_id: Option<Uuid>,
    title: Option<String>,
    prompt: String,
    idempotency_key: String,
}

pub(super) async fn workspace(app: &AppHandle, id: &str) -> Result<WorkspaceInfo, Error> {
    let record = app
        .state::<AppState>()
        .workspace_service
        .require_workspace(id)
        .await
        .map_err(|_| Error(StatusCode::NOT_FOUND, "Workspace unavailable".into()))?;
    if !eligible(&record) {
        return Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Unsupported workspace on this desktop".into(),
        ));
    }
    Ok(record)
}

fn scope(workspace: &WorkspaceInfo) -> DesktopSessionScopeRequest {
    DesktopSessionScopeRequest {
        workspace_id: Some(workspace.id.clone()),
        workspace_path: workspace.root_path.to_string_lossy().into_owned(),
        remote_connection_id: None,
        remote_ssh_host: None,
    }
}

fn acknowledgement(r: &Receipt) -> (StatusCode, Value) {
    let mut body = json!({
        "sessionId": r.session_id, "turnId": r.turn_id,
        "workspaceId": r.workspace_id, "admission": r.admission
    });
    if let Some(error) = &r.error {
        body["error"] = json!(error);
    }
    let status = if r.admission == "failed" {
        StatusCode::CONFLICT
    } else {
        StatusCode::ACCEPTED
    };
    (status, body)
}

pub(super) async fn submit(
    host: Host,
    grant: Grant,
    request: Submit,
) -> Result<(StatusCode, Value), Error> {
    if request.prompt.trim().is_empty()
        || request.idempotency_key.trim().is_empty()
        || request.idempotency_key.len() > 128
        || request.title.as_ref().is_some_and(|title| {
            title.trim().is_empty() || title.chars().count() > 160 || request.session_id.is_some()
        })
    {
        return Err(Error(
            StatusCode::BAD_REQUEST,
            "A prompt and idempotency key are required. Titles apply only to new sessions.".into(),
        ));
    }
    if !grant.workspace_ids.contains(&request.workspace_id) {
        return Err(Error(
            StatusCode::FORBIDDEN,
            "Workspace is outside the grant".into(),
        ));
    }
    let controller = host.controller();
    let _serial = controller.admission.lock().await;
    if !host.active(grant.id).await {
        return Err(Error(
            StatusCode::UNAUTHORIZED,
            "Integration disabled".into(),
        ));
    }
    let target = workspace(&host.app, &request.workspace_id).await?;
    let turn = turn_id(grant.id, &request.idempotency_key);
    let fingerprint = digest(&serde_json::to_vec(&request).map_err(|e| e.to_string())?);
    let store = controller.store()?;
    if let Some(existing) = store.receipt(grant.id, turn)? {
        if existing.fingerprint != fingerprint {
            return Err(Error(
                StatusCode::CONFLICT,
                "Idempotency key was used for a different request".into(),
            ));
        }
        return Ok(acknowledgement(&existing));
    }
    let session = request.session_id.unwrap_or(turn);
    if request.session_id.is_some() {
        // The first turn doubles as an unforgeable session ownership record within this grant.
        let owner = host.receipt(&grant, session).await?;
        if owner.session_id != session || owner.workspace_id != target.id {
            return Err(Error(
                StatusCode::NOT_FOUND,
                "Session not found in this workspace".into(),
            ));
        }
        if host
            .app
            .state::<Arc<DialogScheduler>>()
            .is_session_busy_or_queued(&session.to_string())
        {
            return Err(Error(StatusCode::CONFLICT, "Session is busy".into()));
        }
    }
    let mut receipt = Receipt {
        session_id: session,
        turn_id: turn,
        workspace_id: target.id.clone(),
        fingerprint,
        admission: "outcome_unknown".into(),
        error: None,
        instance: controller.instance,
    };
    store.save_receipt(grant.id, &receipt)?;
    let result = start(&host.app, &request, &target, &receipt).await;
    match result {
        Ok(DialogSubmitOutcome::Queued { .. }) => receipt.admission = "queued".into(),
        Ok(DialogSubmitOutcome::Started { .. }) => receipt.admission = "started".into(),
        Err(error) => {
            if error.kind != openbitfun_runtime_ports::PortErrorKind::OutcomeUnknown {
                receipt.admission = "failed".into();
            }
            receipt.error = Some(error.message);
        }
    }
    store.save_receipt(grant.id, &receipt)?;
    Ok(acknowledgement(&receipt))
}

async fn start(
    app: &AppHandle,
    request: &Submit,
    workspace: &WorkspaceInfo,
    r: &Receipt,
) -> Result<DialogSubmitOutcome, openbitfun_runtime_ports::PortError> {
    use openbitfun_runtime_ports::{PortError, PortErrorKind};
    let path = workspace
        .root_path
        .to_str()
        .ok_or_else(|| {
            PortError::new(
                PortErrorKind::InvalidRequest,
                "Unsupported workspace path encoding",
            )
        })?
        .to_owned();
    let session_id = r.session_id.to_string();
    let context = app.state::<DesktopRuntimeContext>();
    if request.session_id.is_none() {
        let title = request.title.clone().unwrap_or_else(|| {
            request
                .prompt
                .trim()
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect()
        });
        create_desktop_session(
            app.state::<Arc<ConversationCoordinator>>().inner(),
            app.state::<AppState>().inner(),
            &context,
            CreateSessionRequest {
                session_id: Some(session_id.clone()),
                session_name: title,
                agent_type: "Standard".into(),
                workspace_path: path.clone(),
                workspace_id: Some(workspace.id.clone()),
                project_workspace_path: None,
                execution_target: None,
                request_id: None,
                session_kind: None,
                remote_connection_id: None,
                remote_ssh_host: None,
                relationship: None,
                deep_review_run_manifest: None,
                review_target_evidence: None,
                config: None,
            },
        )
        .await
        .map_err(|message| PortError::new(PortErrorKind::Backend, message))?;
    }
    openbitfun_core::product_runtime::idle_session_dialog_port(
        app.state::<Arc<ConversationCoordinator>>().inner().clone(),
        app.state::<Arc<DialogScheduler>>().inner().clone(),
    )
    .submit_dialog_turn(AgentDialogTurnRequest {
        session_id,
        message: request.prompt.clone(),
        original_message: None,
        output_schema: None,
        turn_id: Some(r.turn_id.to_string()),
        execution: Default::default(),
        agent_type: "Standard".into(),
        workspace_path: Some(path),
        workspace_id: Some(workspace.id.clone()),
        remote_connection_id: None,
        remote_ssh_host: None,
        policy: DialogSubmissionPolicy::for_source(AgentSubmissionSource::DesktopApi),
        reply_route: None,
        prepended_reminders: Vec::new(),
        attachments: Vec::new(),
        metadata: Default::default(),
    })
    .await
}

async fn persisted_turn(
    app: &AppHandle,
    workspace: &WorkspaceInfo,
    r: &Receipt,
) -> Result<Option<DialogTurnData>, String> {
    let ctx = app.state::<DesktopRuntimeContext>();
    let id = r.turn_id.to_string();
    let mut request = SessionTurnWindowRequest {
        workspace_path: Default::default(),
        session_id: r.session_id.to_string(),
        include_internal: false,
        target_storage_turn_index: 0,
        expected_turn_id: Some(id.clone()),
        expected_catalog_revision: None,
        before: 0,
        after: 1,
    };
    // The window owner returns a lightweight catalog when its initial index does not match.
    for _ in 0..2 {
        match ctx
            .session_application()
            .load_session_turn_window(scope(workspace), request.clone())
            .await
            .map_err(|e| e.to_string())?
        {
            SessionTurnWindowResponse::Ready { turns, .. } => {
                return Ok(turns.into_iter().find(|t| t.turn_id == id))
            }
            SessionTurnWindowResponse::Stale { catalog }
            | SessionTurnWindowResponse::NotFound { catalog } => {
                let Some(entry) = catalog
                    .entries
                    .iter()
                    .find(|e| e.turn_id.as_deref() == Some(&id))
                else {
                    return Ok(None);
                };
                request.target_storage_turn_index = entry.storage_turn_index;
                request.expected_catalog_revision = Some(catalog.revision);
            }
        }
    }
    Ok(None)
}

pub(super) async fn snapshot(host: &Host, r: &Receipt) -> Result<Value, Error> {
    let app = &host.app;
    let workspace = workspace(app, &r.workspace_id).await?;
    let session = r.session_id.to_string();
    let id = r.turn_id.to_string();
    let ctx = app.state::<DesktopRuntimeContext>();
    let manager = app.state::<Arc<ConversationCoordinator>>();
    let mut value = json!({"sessionId":session,"turnId":id,"workspaceId":r.workspace_id});
    if let Some(SessionState::Processing {
        current_turn_id,
        phase,
    }) = manager.get_session_manager().get_session_state(&session)
    {
        if current_turn_id == id {
            let mailbox = ctx
                .session_application()
                .session_interaction_snapshot(&session);
            value["status"] = json!(if !mailbox.permissions.requests.is_empty() {
                "awaiting_approval"
            } else if !mailbox.user_questions.questions.is_empty() {
                "awaiting_input"
            } else {
                "running"
            });
            value["phase"] = json!(phase);
            return Ok(value);
        }
    }
    if r.admission == "failed" {
        value["status"] = json!("failed");
        value["error"] = json!(r.error);
        return Ok(value);
    }
    // Desktop may have exited after journaling and before creating the session.
    if r.admission == "outcome_unknown"
        && ctx
            .session_application()
            .load_session_metadata(scope(&workspace), &session)
            .await
            .map_err(|e| e.to_string())?
            .is_none()
    {
        value["status"] = json!("outcome_unknown");
        value["error"] = json!(r.error);
        return Ok(value);
    }
    let turn = persisted_turn(app, &workspace, r).await?;
    if let Some(turn) = turn.filter(|t| t.status != TurnStatus::InProgress) {
        value["status"] = json!(match turn.status {
            TurnStatus::Completed => "completed",
            TurnStatus::Cancelled => "cancelled",
            _ => "failed",
        });
        value["error"] = json!(turn.error);
        value["finalReply"] = json!(turn
            .model_rounds
            .last()
            .filter(|_| turn.has_final_response != Some(false))
            .map(|round| round
                .text_items
                .iter()
                .map(|item| item.content.as_str())
                .collect::<String>()));
    } else {
        value["status"] = json!(if r.instance == host.controller().instance
            && r.admission == "queued"
        {
            "queued"
        } else {
            "outcome_unknown"
        });
    }
    Ok(value)
}

pub(super) async fn cancel(app: &AppHandle, r: &Receipt) -> Result<Value, Error> {
    let result = app
        .state::<DesktopRuntimeContext>()
        .agent_runtime()
        .cancel_turn(AgentTurnCancellationRequest {
            session_id: r.session_id.to_string(),
            turn_id: Some(r.turn_id.to_string()),
            source: Some(AgentSubmissionSource::DesktopApi),
            requester_session_id: None,
            reason: None,
            wait_timeout_ms: None,
            cancel_descendants: false,
        })
        .await
        .map_err(|e| Error(StatusCode::CONFLICT, e.into_message()))?;
    Ok(json!(result))
}
