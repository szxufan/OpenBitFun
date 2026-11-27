//! Deletion intents live outside both the chat and its snapshot history.
//! Replay uses the same disk reference scan and writer locks as manual cleanup.
use super::*;
use crate::service::snapshot::{snapshot_core::sanitize_id, storage::write_atomic};
use sha2::{Digest, Sha256};

#[derive(Deserialize, Serialize)]
pub(crate) struct PendingDeletion {
    workspace_id: String,
    store_key: String,
    session_id: String,
}

// Bind the intent to both stores. A reconfigured worktree or SSH connection
// can keep its workspace ID while resolving to different on-disk data.
fn store_key(projects: &Path, scope: &Scope) -> SnapshotResult<String> {
    let identity = (
        &scope.id,
        scope
            .context
            .snapshots_dir
            .strip_prefix(projects)
            .map_err(invalid)?,
        scope.sessions.strip_prefix(projects).map_err(invalid)?,
    );
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&identity)?)
    ))
}

impl PendingDeletion {
    fn new(projects: &Path, scope: &Scope, id: &str) -> SnapshotResult<Self> {
        Ok(Self {
            workspace_id: scope.id.clone(),
            store_key: store_key(projects, scope)?,
            session_id: id.to_owned(),
        })
    }

    fn directory(projects: &Path) -> PathBuf {
        projects.with_file_name("pending-snapshot-deletions")
    }

    fn path(&self, projects: &Path) -> SnapshotResult<PathBuf> {
        let digest = Sha256::digest(serde_json::to_vec(self)?);
        Ok(Self::directory(projects).join(format!("{digest:x}.json")))
    }

    pub(super) fn prepare(
        projects: &Path,
        scope: &Scope,
        session_id: &str,
    ) -> SnapshotResult<Option<Self>> {
        let history = history_path(scope, session_id);
        checked_path(&scope.context.snapshots_dir, &history)?;
        if missing(&history)? {
            return Ok(None);
        }
        let intent = Self::new(projects, scope, session_id)?;
        let path = intent.path(projects)?;
        validate_journal_path(projects, &path)?;
        fs::create_dir_all(Self::directory(projects))?;
        write_atomic(&path, &serde_json::to_vec(&intent)?)?;
        Ok(Some(intent))
    }

    pub(crate) async fn finish(self, paths: &PathManager) -> SnapshotResult<()> {
        let scope = registered_scope(paths, &self.workspace_id).await?;
        let projects = paths.projects_root();
        if store_key(&projects, &scope)? != self.store_key {
            return Err(invalid(
                "Snapshot store identity changed after session deletion",
            ));
        }
        let result = clean_scope(scope, Cleanup::Pending(vec![self.session_id]), projects).await?;
        if result.errors.is_empty() && result.skipped_sessions == 0 {
            Ok(())
        } else {
            Err(invalid(format!(
                "Snapshot cleanup deferred: {} skipped, {:?}",
                result.skipped_sessions, result.errors
            )))
        }
    }
}

fn history_path(scope: &Scope, session_id: &str) -> PathBuf {
    scope
        .context
        .snapshot_operations_dir
        .join(format!("{}.json", sanitize_id(session_id)))
}

fn missing(path: &Path) -> SnapshotResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e.into()),
    }
}

pub(crate) async fn prepare_session_deletion(
    paths: &PathManager,
    workspace_id: Option<&str>,
    sessions: &Path,
    session_id: &str,
) -> SnapshotResult<Option<PendingDeletion>> {
    openbitfun_core_types::validate_session_id(session_id).map_err(invalid)?;
    if crate::service::workspace::get_global_workspace_service().is_none() {
        return Ok(None);
    }
    let scopes = known_scopes(paths).await?;
    let actual = dunce::canonicalize(sessions)?;
    let mut matches = scopes.into_values().flatten().filter(|s| {
        workspace_id.is_none_or(|id| s.id == id)
            && dunce::canonicalize(&s.sessions).is_ok_and(|p| p == actual)
    });
    let Some(scope) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Ok(None);
    }
    let root = paths.projects_root();
    let id = session_id.to_owned();
    tokio::task::spawn_blocking(move || {
        checked_path(&root, &scope.context.snapshots_dir)?;
        checked_path(&root, &scope.context.locks_dir)?;
        checked_path(&root, &scope.sessions)?;
        PendingDeletion::prepare(&root, &scope, &id)
    })
    .await
    .map_err(invalid)?
}

// A crash after unlinking the history but before acknowledging the intent is
// harmless. An intent for a surviving session can also be retired under its lock.
pub(super) fn acknowledge<'a>(
    projects: &Path,
    scope: &Scope,
    ids: impl Iterator<Item = &'a str>,
) -> SnapshotResult<()> {
    for id in ids {
        openbitfun_core_types::validate_session_id(id).map_err(invalid)?;
        let intent = PendingDeletion::new(projects, scope, id)?;
        let path = intent.path(projects)?;
        validate_journal_path(projects, &path)?;
        if missing(&path)? {
            continue;
        }
        if !missing(&history_path(scope, id))? {
            // An interrupted or failed deletion can leave both the chat and the intent.
            // Retire that intent only after the original deleting writer has gone away.
            if !fs::symlink_metadata(&scope.sessions).is_ok_and(|m| m.is_dir()) {
                continue;
            }
            let Ok(_guard) =
                openbitfun_services_core::session::SessionWriteLock::try_acquire_for_maintenance(
                    &scope.sessions,
                    id,
                )
            else {
                continue;
            };
            if !fs::symlink_metadata(scope.sessions.join(id)).is_ok_and(|m| m.is_dir()) {
                continue;
            }
            fs::remove_file(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn validate_journal_path(projects: &Path, path: &Path) -> SnapshotResult<()> {
    checked_path(
        projects
            .parent()
            .ok_or_else(|| invalid("Invalid project storage root"))?,
        path,
    )
}

pub(super) fn pending_by_store(projects: &Path) -> SnapshotResult<BTreeMap<String, Vec<String>>> {
    let mut pending = BTreeMap::<String, Vec<String>>::new();
    for entry in entries(&PendingDeletion::directory(projects))? {
        let path = entry.path();
        if !entry.file_type()?.is_file() || path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        match plan::json::<PendingDeletion>(&path) {
            Ok(intent)
                if intent.path(projects)? == path
                    && openbitfun_core_types::validate_session_id(&intent.session_id).is_ok() =>
            {
                pending
                    .entry(intent.store_key)
                    .or_default()
                    .push(intent.session_id);
            }
            _ => log::warn!(
                "Leaving unrecognized snapshot deletion intent in {}",
                path.display()
            ),
        }
    }
    Ok(pending)
}

pub(crate) async fn retry_pending_deletions(paths: &PathManager) -> SnapshotResult<()> {
    let projects = paths.projects_root();
    let root = projects.clone();
    let mut pending = tokio::task::spawn_blocking(move || pending_by_store(&root))
        .await
        .map_err(invalid)??;
    if pending.is_empty() {
        return Ok(());
    }
    for scope in known_scopes(paths).await?.into_values().flatten() {
        let id = scope.id.clone();
        let Some(session_ids) = pending.remove(&store_key(&projects, &scope)?) else {
            continue;
        };
        match clean_scope(scope, Cleanup::Pending(session_ids), projects.clone()).await {
            Ok(result) if result.errors.is_empty() => {}
            Ok(result) => log::warn!("Pending snapshot cleanup for {id}: {:?}", result.errors),
            Err(e) => log::warn!("Pending snapshot cleanup for {id} deferred: {e}"),
        }
    }
    Ok(())
}
