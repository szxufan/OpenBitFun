//! Opt-in loopback control of the Runtime owned by this desktop instance.
mod http;
mod store;
mod turns;

use crate::api::app_state::AppState;
use openbitfun_core::service::workspace::{WorkspaceInfo, WorkspaceKind};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};
use store::{Grant, Settings, Store};
use tauri::{AppHandle, Manager};
use tokio::{
    net::TcpListener,
    sync::{oneshot, Mutex},
};
use uuid::Uuid;

pub(super) const PORT: u16 = 38371;

struct Listener {
    generation: Uuid,
    _shutdown: oneshot::Sender<()>,
}

struct Inner {
    settings: Settings,
    listener: Option<Listener>,
    listener_error: Option<String>,
}

pub(crate) struct DesktopController {
    storage: Result<Store, String>,
    inner: Mutex<Inner>,
    instance: Uuid,
    admission: Mutex<()>,
}

impl DesktopController {
    pub(crate) fn new(root: PathBuf) -> Arc<Self> {
        let source = Store::open(root).and_then(|store| Ok((store.settings()?, store)));
        let (settings, storage) = match source {
            Ok((settings, store)) => (settings, Ok(store)),
            Err(e) => (Settings::default(), Err(e)),
        };
        Arc::new(Self {
            storage,
            inner: Mutex::new(Inner {
                settings,
                listener: None,
                listener_error: None,
            }),
            instance: Uuid::new_v4(),
            admission: Mutex::new(()),
        })
    }

    fn store(&self) -> Result<&Store, String> {
        self.storage.as_ref().map_err(Clone::clone)
    }

    async fn listen(&self, app: AppHandle, port: u16) -> Result<Listener, String> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|e| format!("Cannot listen on 127.0.0.1:{port}: {e}"))?;
        let generation = Uuid::new_v4();
        let router = http::router(app.clone(), generation, port);
        let (shutdown, end) = oneshot::channel();
        tauri::async_runtime::spawn(async move {
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = end.await;
                })
                .await;
            let controller = app.state::<Arc<DesktopController>>();
            let mut inner = controller.inner.lock().await;
            if inner
                .listener
                .as_ref()
                .is_some_and(|l| l.generation == generation)
            {
                inner.listener = None;
                inner.listener_error = Some(
                    result
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "Controller stopped".into()),
                );
            }
        });
        Ok(Listener {
            generation,
            _shutdown: shutdown,
        })
    }
}

pub(crate) async fn restore(app: AppHandle) {
    let host = app.state::<Arc<DesktopController>>();
    let mut inner = host.inner.lock().await;
    if inner.settings.enabled && inner.listener.is_none() {
        match host
            .listen(app.clone(), inner.settings.port.unwrap_or(PORT))
            .await
        {
            Ok(listener) => inner.listener = Some(listener),
            Err(error) => {
                log::warn!("Desktop external controller unavailable: {error}");
                inner.listener_error = Some(error);
            }
        }
    }
}

fn eligible(w: &WorkspaceInfo) -> bool {
    w.workspace_kind == WorkspaceKind::Normal && w.worktree.as_ref().is_none_or(|tree| tree.is_main)
}

async fn workspaces(app: &AppHandle) -> Vec<WorkspaceInfo> {
    app.state::<AppState>()
        .workspace_service
        .list_workspace_infos()
        .await
        .into_iter()
        .filter(eligible)
        .collect()
}

#[derive(Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ControllerSettingsRequest {
    Status,
    SetEnabled {
        enabled: bool,
        port: Option<std::num::NonZeroU16>,
    },
    Issue {
        name: String,
        workspace_ids: BTreeSet<String>,
        origin: Option<String>,
    },
    Revoke {
        id: Uuid,
    },
}

fn browser_origin(value: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid browser origin")?;
    let origin = url.origin().ascii_serialization();
    if !matches!(url.scheme(), "https" | "http") || origin != value.trim_end_matches('/') {
        return Err("Enter an exact HTTP or HTTPS origin".into());
    }
    Ok(origin)
}

#[tauri::command]
pub(crate) async fn desktop_external_control(
    webview: tauri::Webview,
    app: AppHandle,
    request: ControllerSettingsRequest,
) -> Result<Value, String> {
    if webview.label() != "main" {
        return Err("Desktop controller settings require the main webview".into());
    }
    let host = app.state::<Arc<DesktopController>>();
    let store = host.store()?;
    let catalog = workspaces(&app).await;
    let mut inner = host.inner.lock().await;
    let mut next = inner.settings.clone();
    let mut token = None;
    match request {
        ControllerSettingsRequest::Status => {}
        ControllerSettingsRequest::SetEnabled { enabled, port } => {
            let port = port.map(|p| p.get()).or(next.port).unwrap_or(PORT);
            let listener =
                if enabled && (inner.listener.is_none() || next.port.unwrap_or(PORT) != port) {
                    Some(host.listen(app.clone(), port).await?)
                } else {
                    None
                };
            next.enabled = enabled;
            next.port = Some(port);
            store.save_settings(&next)?;
            if !enabled {
                inner.listener = None;
            } else if listener.is_some() {
                inner.listener = listener;
            }
            inner.listener_error = None;
        }
        ControllerSettingsRequest::Issue {
            name,
            workspace_ids,
            origin,
        } => {
            if name.trim().is_empty()
                || name.len() > 128
                || workspace_ids.is_empty()
                || !workspace_ids
                    .iter()
                    .all(|id| catalog.iter().any(|w| &w.id == id))
            {
                return Err("Select a local workspace and enter an integration name".into());
            }
            let origin = origin
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(browser_origin)
                .transpose()?;
            let secret = format!("obf_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
            next.grants.push(Grant {
                id: Uuid::new_v4(),
                name: name.trim().into(),
                workspace_ids,
                origin,
                token_hash: store::digest(secret.as_bytes()),
            });
            store.save_settings(&next)?;
            token = Some(secret);
        }
        ControllerSettingsRequest::Revoke { id } => {
            next.grants.retain(|g| g.id != id);
            store.save_settings(&next)?;
        }
    }
    inner.settings = next;
    Ok(json!({
        "enabled": inner.settings.enabled, "listening": inner.listener.is_some(),
        "port": inner.settings.port.unwrap_or(PORT),
        "endpoint": format!("http://127.0.0.1:{}/v1", inner.settings.port.unwrap_or(PORT)), "error": inner.listener_error,
        "token": token,
        "grants": inner.settings.grants.iter().map(|g| json!({"id":g.id, "name":g.name, "workspaceIds":g.workspace_ids, "origin":g.origin})).collect::<Vec<_>>(),
        "workspaces": catalog.iter().map(|w| json!({"id":w.id, "name":w.name})).collect::<Vec<_>>()
    }))
}
