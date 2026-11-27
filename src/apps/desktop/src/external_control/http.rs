use super::{
    store::{Grant, Receipt},
    turns, workspaces, DesktopController,
};
use axum::{
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::{get, post},
    Extension, Json, Router,
};
use serde_json::{json, Value};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

#[derive(Clone)]
pub(super) struct Host {
    pub app: AppHandle,
    generation: Uuid,
    authority: String,
}

pub(super) struct Error(pub StatusCode, pub String);
impl From<String> for Error {
    fn from(message: String) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

impl Host {
    pub(super) fn controller(&self) -> tauri::State<'_, Arc<DesktopController>> {
        self.app.state()
    }

    pub(super) async fn active(&self, id: Uuid) -> bool {
        let controller = self.controller();
        let inner = controller.inner.lock().await;
        inner
            .listener
            .as_ref()
            .is_some_and(|l| l.generation == self.generation)
            && inner.settings.grants.iter().any(|g| g.id == id)
    }

    pub(super) async fn receipt(&self, grant: &Grant, id: Uuid) -> Result<Receipt, Error> {
        let receipt = self
            .controller()
            .store()?
            .receipt(grant.id, id)?
            .filter(|r| grant.workspace_ids.contains(&r.workspace_id))
            .ok_or_else(|| Error(StatusCode::NOT_FOUND, "Turn not found".into()))?;
        turns::workspace(&self.app, &receipt.workspace_id).await?;
        Ok(receipt)
    }
}

pub(super) fn router(app: AppHandle, generation: Uuid, port: u16) -> Router {
    let host = Host {
        app,
        generation,
        authority: format!("127.0.0.1:{port}"),
    };
    Router::new()
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/workspaces", get(catalog))
        .route("/v1/turns", post(submit))
        .route("/v1/turns/{id}", get(status))
        .route("/v1/turns/{id}/events", get(events))
        .route("/v1/turns/{id}/cancel", post(cancel))
        .route("/v1/turns/{id}/open", post(open))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(host.clone(), authorize))
        .with_state(host)
}

async fn authorize(State(host): State<Host>, mut request: Request, next: Next) -> Response {
    let headers = request.headers();
    if headers.get(header::HOST).and_then(|v| v.to_str().ok()) != Some(host.authority.as_str()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let origin = headers.get(header::ORIGIN).cloned();
    let origin_str = match origin.as_ref().map(HeaderValue::to_str).transpose() {
        Ok(value) => value,
        Err(_) => return StatusCode::FORBIDDEN.into_response(),
    };
    let controller = host.controller();
    let inner = controller.inner.lock().await;
    if !inner
        .listener
        .as_ref()
        .is_some_and(|l| l.generation == host.generation)
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let mut response = if request.method() == Method::OPTIONS {
        if origin_str.is_none()
            || !inner
                .settings
                .grants
                .iter()
                .any(|g| g.origin.as_deref() == origin_str)
        {
            return StatusCode::FORBIDDEN.into_response();
        }
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Authorization, Content-Type"),
        );
        response.headers_mut().insert(
            "access-control-allow-private-network",
            HeaderValue::from_static("true"),
        );
        response
    } else {
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));
        let Some(grant) = token
            .and_then(|token| {
                inner
                    .settings
                    .grants
                    .iter()
                    .find(|g| g.accepts(token, origin_str))
            })
            .cloned()
        else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        drop(inner);
        request.extensions_mut().insert(grant);
        next.run(request).await
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(origin) = origin {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        response
            .headers_mut()
            .append(header::VARY, HeaderValue::from_static("Origin"));
    }
    response
}

async fn capabilities() -> Json<Value> {
    Json(
        json!({"version":1,"executionHost":"desktop","workspaceKinds":["localProject"],
        "operations":["submit","continue","status","events","cancel","open"],"approvalSurface":"desktop"}),
    )
}

async fn catalog(State(host): State<Host>, Extension(grant): Extension<Grant>) -> Json<Value> {
    Json(
        json!({"workspaces":workspaces(&host.app).await.into_iter().filter(|w| grant.workspace_ids.contains(&w.id))
        .map(|w| json!({"id":w.id,"name":w.name})).collect::<Vec<_>>() }),
    )
}

async fn submit(
    State(host): State<Host>,
    Extension(grant): Extension<Grant>,
    Json(request): Json<turns::Submit>,
) -> Result<impl IntoResponse, Error> {
    // A dropped HTTP connection must not cancel admission between the persisted receipt and Runtime handoff.
    let result =
        tauri::async_runtime::spawn(async move { turns::submit(host, grant, request).await })
            .await
            .map_err(|e| Error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok((result.0, Json(result.1)))
}

async fn status(
    State(host): State<Host>,
    Extension(grant): Extension<Grant>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, Error> {
    let receipt = host.receipt(&grant, id).await?;
    Ok(Json(turns::snapshot(&host, &receipt).await?))
}

async fn events(
    State(host): State<Host>,
    Extension(grant): Extension<Grant>,
    Path(id): Path<Uuid>,
) -> Result<Response, Error> {
    let receipt = host.receipt(&grant, id).await?;
    // Every attachment gets a fresh full snapshot. No client cursor or replay buffer is needed.
    let stream = futures::stream::unfold(
        (host, grant, receipt, None::<Value>),
        |(host, grant, receipt, mut previous)| async move {
            if previous
                .as_ref()
                .and_then(|v| v["status"].as_str())
                .is_some_and(|s| matches!(s, "completed" | "cancelled" | "failed"))
            {
                return None;
            }
            loop {
                if previous.is_some() {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                if !host.active(grant.id).await {
                    return None;
                }
                let current = match turns::snapshot(&host, &receipt).await {
                    Ok(value) => value,
                    Err(error) => json!({"status":"unavailable","error":error.1}),
                };
                if previous.as_ref() != Some(&current) {
                    let event = Event::default().event("snapshot").data(current.to_string());
                    previous = Some(current);
                    return Some((Ok::<_, Infallible>(event), (host, grant, receipt, previous)));
                }
            }
        },
    );
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

async fn cancel(
    State(host): State<Host>,
    Extension(grant): Extension<Grant>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, Error> {
    let receipt = host.receipt(&grant, id).await?;
    Ok(Json(turns::cancel(&host.app, &receipt).await?))
}

async fn open(
    State(host): State<Host>,
    Extension(grant): Extension<Grant>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, Error> {
    let receipt = host.receipt(&grant, id).await?;
    crate::appearance::show_main_window(host.app.clone()).await?;
    host.app.emit_to(tauri::EventTarget::webview("main"), "desktop-control://open-session",
        json!({"sessionId":receipt.session_id,"workspaceId":receipt.workspace_id,"surfaceId":"local"})).map_err(|e| e.to_string())?;
    Ok(StatusCode::ACCEPTED)
}
