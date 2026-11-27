//! Controller-local file snapshot accounting. All deletion paths originate in
//! a directory enumeration of the app store, never in a request or descriptor.
mod pending;
mod plan;
pub(crate) use pending::{prepare_session_deletion, retry_pending_deletions};
mod writer_lease;
pub(super) use writer_lease::{ViewStamp, WriterLease};

use super::manager::{bound_snapshot_context, maintenance_manager};
use super::types::{SnapshotError, SnapshotResult};
use crate::agentic::WorkspaceBinding;
use crate::infrastructure::PathManager;
use crate::service::workspace_runtime::{WorkspaceRuntimeContext, WorkspaceRuntimeTarget};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Period {
    SevenDays,
    ThirtyDays,
    All,
}

impl Period {
    fn includes(self, active: u64, now: u64) -> bool {
        let days = match self {
            Self::SevenDays => 7,
            Self::ThirtyDays => 30,
            Self::All => return true,
        };
        active < now.saturating_sub(days * 86_400_000)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub session_id: String,
    pub name: String,
    pub active_at: u64,
    pub revision: String,
    #[serde(default)]
    pub removed: bool,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceUsage {
    workspace_id: String,
    name: String,
    location: String,
    bytes: u64,
    shared_bytes: u64,
    estimated_bytes: u64,
    removed_estimated_bytes: u64,
    skipped_sessions: usize,
    candidates: Vec<Candidate>,
    warning: Option<String>,
    unverified_references: bool,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageOverview {
    total_bytes: u64,
    unidentified_bytes: u64,
    workspaces: Vec<WorkspaceUsage>,
    scan_errors: Vec<String>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    removed_sessions: usize,
    skipped_sessions: usize,
    freed_bytes: u64,
    errors: Vec<String>,
}

#[derive(Default)]
pub(super) struct Removed {
    pub sessions: HashSet<String>,
    pub handles: HashSet<String>,
    pub blobs: HashSet<String>,
}

struct Scope {
    id: String,
    name: String,
    location: String,
    remote: bool,
    context: WorkspaceRuntimeContext,
    sessions: PathBuf,
}

fn invalid(message: impl ToString) -> SnapshotError {
    SnapshotError::ConfigError(message.to_string())
}

fn millis(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

async fn known_scopes(paths: &PathManager) -> SnapshotResult<BTreeMap<PathBuf, Option<Scope>>> {
    let service = crate::service::workspace::get_global_workspace_service()
        .ok_or_else(|| invalid("Workspace service unavailable"))?;
    let mut scopes = BTreeMap::new();
    for record in service.list_workspace_infos().await {
        let Ok(binding) = WorkspaceBinding::resolve(&record.id).await else {
            continue;
        };
        let identity = &binding.session_identity;
        let remote = identity.is_remote();
        // Opening Storage must not run the normal on-demand runtime migration.
        let context = if remote {
            use openbitfun_services_core::workspace_identity::{
                normalize_remote_workspace_path, remote_workspace_runtime_key,
                remote_workspace_runtime_root,
            };
            let host = identity.identity_host();
            let remote_root = normalize_remote_workspace_path(&identity.logical_workspace_path);
            let legacy = remote_workspace_runtime_root(
                paths.remote_ssh_mirror_root_dir(),
                &host,
                &remote_root,
            );
            if legacy.join("snapshots").exists() || legacy.join("sessions").exists() {
                continue;
            }
            let root = paths
                .projects_root()
                .join(remote_workspace_runtime_key(&host, &remote_root));
            WorkspaceRuntimeContext::new(
                WorkspaceRuntimeTarget::RemoteWorkspaceMirror {
                    ssh_host: host,
                    remote_root,
                },
                root,
            )
        } else {
            WorkspaceRuntimeContext::new(
                WorkspaceRuntimeTarget::LocalWorkspace {
                    workspace_root: binding.root_path.clone(),
                },
                paths.project_runtime_location(&binding.root_path),
            )
        };
        let Ok(context) = bound_snapshot_context(identity, context) else {
            continue;
        };
        let sessions = if !remote && binding.project_root_path != binding.root_path {
            paths
                .project_runtime_location(&binding.project_root_path)
                .join("sessions")
        } else {
            context.sessions_dir.clone()
        };
        let location = if remote {
            format!(
                "{} · {} · {}",
                identity.hostname,
                identity.logical_workspace_path,
                identity.remote_connection_id.as_deref().unwrap_or_default()
            )
        } else {
            identity.logical_workspace_path.clone()
        };
        let key = context.snapshots_dir.clone();
        let scope = Scope {
            id: record.id,
            name: record.name,
            remote,
            context,
            sessions,
            location,
        };
        scopes
            .entry(key)
            .and_modify(|v| *v = None)
            .or_insert(Some(scope));
    }
    Ok(scopes)
}

// Ignore symbolic links, including directory links. An unreadable directory
// is an error, not evidence that a blob has no owners.
fn entries(dir: &Path) -> SnapshotResult<Vec<fs::DirEntry>> {
    match fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => Ok(fs::read_dir(dir)?.collect::<Result<_, _>>()?),
        Ok(_) => Err(invalid(format!(
            "Expected a storage directory: {}",
            dir.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn checked_path(root: &Path, path: &Path) -> SnapshotResult<()> {
    let mut current = root.to_owned();
    for part in path.strip_prefix(root).map_err(invalid)?.components() {
        let std::path::Component::Normal(part) = part else {
            return Err(invalid("Invalid storage path"));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(m) if m.is_symlink() => {
                return Err(invalid(format!(
                    "Linked storage path: {}",
                    current.display()
                )))
            }
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}

fn tree_bytes(root: &Path, omit_scopes: bool) -> SnapshotResult<u64> {
    let mut pending = vec![root.to_owned()];
    let mut bytes = 0;
    while let Some(dir) = pending.pop() {
        for entry in entries(&dir)? {
            let kind = entry.file_type()?;
            if kind.is_file() {
                bytes += entry.metadata()?.len();
            }
            if kind.is_dir()
                && !(omit_scopes && dir == root && entry.file_name() == "workspace-identities")
            {
                pending.push(entry.path());
            }
        }
    }
    Ok(bytes)
}

pub async fn inspect(period: Period) -> SnapshotResult<StorageOverview> {
    let paths = crate::infrastructure::try_get_path_manager_arc()?;
    let known = known_scopes(&paths).await?;
    tokio::task::spawn_blocking(move || {
        let mut result = inspect_roots(&paths.projects_root(), known, period)?;
        inspect_legacy_mirrors(&paths.remote_ssh_mirror_root_dir(), &mut result);
        Ok(result)
    })
    .await
    .map_err(invalid)?
}

// Legacy remote runtimes mirror the host and POSIX workspace path at arbitrary
// depth. Recognize the file snapshot layout before counting a directory.
fn inspect_legacy_mirrors(root: &Path, result: &mut StorageOverview) {
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        let children = match entries(&dir) {
            Ok(children) => children,
            Err(e) => {
                result.scan_errors.push(e.to_string());
                continue;
            }
        };
        for child in children {
            let path = child.path();
            match child.file_type() {
                Ok(kind) if kind.is_dir() => {}
                Err(e) => {
                    result.scan_errors.push(e.to_string());
                    continue;
                }
                _ => continue,
            }
            if child.file_name() == "snapshots"
                && [
                    "metadata",
                    "baselines",
                    "operations",
                    "by_hash",
                    "workspace-identities",
                ]
                .iter()
                .any(|s| path.join(s).is_dir())
            {
                match tree_bytes(&path, false) {
                    Ok(bytes) => {
                        result.total_bytes += bytes;
                        result.unidentified_bytes += bytes;
                    }
                    Err(e) => result
                        .scan_errors
                        .push(format!("{}: {}", path.display(), e)),
                }
            } else {
                pending.push(path);
            }
        }
    }
}

fn inspect_roots(
    root: &Path,
    mut known: BTreeMap<PathBuf, Option<Scope>>,
    period: Period,
) -> SnapshotResult<StorageOverview> {
    let mut overview = StorageOverview::default();
    for project in entries(root)? {
        if !project.file_type()?.is_dir() {
            continue;
        }
        let base = project.path().join("snapshots");
        if let Err(e) = checked_path(root, &base) {
            overview.scan_errors.push(e.to_string());
            continue;
        }
        let mut dirs = vec![base.clone()];
        match entries(&base.join("workspace-identities")) {
            Ok(children) => {
                for child in children {
                    match child.file_type() {
                        Ok(kind) if kind.is_dir() => dirs.push(child.path()),
                        Ok(kind) if kind.is_file() => match child.metadata() {
                            Ok(meta) => {
                                overview.unidentified_bytes += meta.len();
                                overview.total_bytes += meta.len();
                            }
                            Err(e) => overview.scan_errors.push(e.to_string()),
                        },
                        Ok(_) => {}
                        Err(e) => overview.scan_errors.push(e.to_string()),
                    }
                }
            }
            Err(e) => overview.scan_errors.push(e.to_string()),
        }
        for dir in dirs {
            let bytes = match tree_bytes(&dir, dir == base) {
                Ok(bytes) => bytes,
                Err(e) => {
                    overview
                        .scan_errors
                        .push(format!("{}: {}", dir.display(), e));
                    continue;
                }
            };
            overview.total_bytes += bytes;
            let Some(Some(scope)) = known.remove(&dir) else {
                overview.unidentified_bytes += bytes;
                continue;
            };
            let mut row = WorkspaceUsage {
                bytes,
                workspace_id: scope.id.clone(),
                name: scope.name.clone(),
                location: scope.location.clone(),
                ..Default::default()
            };
            match checked_path(root, &scope.sessions)
                .and_then(|_| plan::Plan::load(&scope, period, None))
            {
                Ok(plan) => {
                    row.shared_bytes = bytes.saturating_sub(plan.single_session_bytes);
                    row.estimated_bytes = plan.estimate();
                    row.removed_estimated_bytes = plan.removed_estimated_bytes;
                    row.skipped_sessions = plan.skipped;
                    row.unverified_references = plan.unverified_references;
                    row.candidates = plan.candidates.into_values().collect();
                }
                Err(e) => row.warning = Some(e.to_string()),
            }
            overview.workspaces.push(row);
        }
    }
    overview
        .workspaces
        .sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    Ok(overview)
}

pub async fn clean(
    workspace_id: &str,
    period: Period,
    confirmed: Vec<Candidate>,
) -> SnapshotResult<CleanupResult> {
    let paths = crate::infrastructure::try_get_path_manager_arc()?;
    let scope = registered_scope(&paths, workspace_id).await?;
    clean_scope(
        scope,
        Cleanup::Confirmed(period, confirmed),
        paths.projects_root(),
    )
    .await
}

enum Cleanup {
    Confirmed(Period, Vec<Candidate>),
    Pending(Vec<String>),
}

async fn registered_scope(paths: &PathManager, workspace_id: &str) -> SnapshotResult<Scope> {
    known_scopes(paths)
        .await?
        .into_values()
        .flatten()
        .find(|s| s.id == workspace_id)
        .ok_or_else(|| invalid("Snapshot store has no unique registered workspace owner"))
}

async fn clean_scope(
    scope: Scope,
    cleanup: Cleanup,
    root: PathBuf,
) -> SnapshotResult<CleanupResult> {
    let (registration, manager) = maintenance_manager(&scope.id, scope.remote).await;
    tokio::task::spawn_blocking(move || {
        let _registration = registration;
        checked_path(&root, &scope.context.snapshots_dir)?;
        checked_path(&root, &scope.sessions)?;
        checked_path(&root, &scope.context.locks_dir)?;
        let service = manager.map(|m| m.get_snapshot_service());
        let service = service.as_ref().map(|s| s.blocking_write());
        let own = service.as_ref().and_then(|s| s.writer_lease.as_ref());
        let _exclusive = writer_lease::exclusive(&scope.context.locks_dir, own)?;
        let mut core = service.as_ref().map(|s| s.snapshot_core.blocking_write());
        let (mut plan, requested, deleted_ids): (_, _, Vec<&str>) = match &cleanup {
            Cleanup::Confirmed(period, candidates) => (
                plan::Plan::load(&scope, *period, Some(candidates))?,
                candidates.len(),
                candidates
                    .iter()
                    .filter(|c| c.removed)
                    .map(|c| c.session_id.as_str())
                    .collect(),
            ),
            Cleanup::Pending(ids) => (
                plan::Plan::for_deleted_sessions(&scope, ids)?,
                ids.len(),
                ids.iter().map(String::as_str).collect(),
            ),
        };
        let (mut result, removed) = plan.remove(requested);
        if let Some(core) = &mut core {
            core.forget_cleaned_history(&removed);
        }
        if let Err(e) = pending::acknowledge(&root, &scope, deleted_ids.into_iter()) {
            result.errors.push(e.to_string());
        }
        Ok(result)
    })
    .await
    .map_err(invalid)?
}

#[cfg(test)]
mod tests;
