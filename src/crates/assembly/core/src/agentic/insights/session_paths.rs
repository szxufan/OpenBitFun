//! Resolve on-disk session roots for insights (local + remote SSH mirror).

use crate::agentic::session::session_store_port::CoreSessionStorePort;
use crate::service::workspace::get_global_workspace_service;
use openbitfun_runtime_ports::SessionStorePort;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct EffectiveSessionStorageTarget {
    pub workspace_id: String,
    pub workspace_path: PathBuf,
    pub session_storage_path: PathBuf,
}

/// Resolve project owners rather than scanning each execution worktree.
/// A failed resolution is never replaced by the logical execution directory.
pub async fn collect_effective_session_storage_targets(
) -> crate::OpenBitFunResult<Vec<EffectiveSessionStorageTarget>> {
    let service = get_global_workspace_service()
        .ok_or_else(|| crate::OpenBitFunError::service("Workspace service is unavailable"))?;
    let workspaces = service.list_workspace_infos().await;
    let project_ids = workspaces
        .iter()
        .map(|workspace| workspace.project_workspace_id())
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(crate::OpenBitFunError::service)?;
    let mut targets = BTreeMap::new();
    let store = CoreSessionStorePort::default();
    for project_id in project_ids {
        let resolution = store
            .resolve_workspace_storage(project_id)
            .await
            .map_err(|error| crate::OpenBitFunError::service(error.to_string()))?;
        if resolution.effective_storage_path.is_dir() {
            insert_storage_target(
                &mut targets,
                EffectiveSessionStorageTarget {
                    workspace_id: project_id.to_owned(),
                    workspace_path: resolution.requested_workspace_path,
                    session_storage_path: resolution.effective_storage_path,
                },
            );
        }
    }
    Ok(targets.into_values().collect())
}

fn insert_storage_target(
    targets: &mut BTreeMap<PathBuf, EffectiveSessionStorageTarget>,
    target: EffectiveSessionStorageTarget,
) {
    targets
        .entry(target.session_storage_path.clone())
        .or_insert(target);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_storage_is_scanned_once_and_remote_mirrors_remain_distinct() {
        let mut targets = BTreeMap::new();
        for (id, workspace, storage) in [
            ("project", "/repo", "/runtime/project/sessions"),
            ("worktree", "/worktrees/branch", "/runtime/project/sessions"),
            ("ssh-a", "/repo", "/runtime/ssh-a/sessions"),
            ("ssh-b", "/repo", "/runtime/ssh-b/sessions"),
        ] {
            insert_storage_target(
                &mut targets,
                EffectiveSessionStorageTarget {
                    workspace_id: id.into(),
                    workspace_path: workspace.into(),
                    session_storage_path: storage.into(),
                },
            );
        }
        assert_eq!(targets.len(), 3);
        assert_eq!(
            targets[&PathBuf::from("/runtime/project/sessions")].workspace_id,
            "project"
        );
    }
}
