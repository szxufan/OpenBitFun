use super::*;
use crate::service::snapshot::{
    snapshot_core::sanitize_id,
    storage::{descriptor, read_bounded, CONTENT_LIMIT},
};
use openbitfun_services_core::session::{
    SessionLastTurn, SessionWriteLock, TurnStatus, SESSION_STORAGE_SCHEMA_VERSION,
};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::fs::File;

#[derive(Deserialize)]
struct History {
    session_id: String,
    last_updated: SystemTime,
    turns: BTreeMap<usize, Turn>,
}
#[derive(Deserialize)]
struct Turn {
    operations: Vec<Operation>,
}
#[derive(Deserialize)]
struct Operation {
    before_snapshot_id: Option<String>,
    after_snapshot_id: Option<String>,
    completed: Option<bool>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Activity {
    #[serde(default, alias = "schema_version")]
    schema_version: u32,
    #[serde(alias = "session_id")]
    session_id: String,
    #[serde(alias = "last_active_at")]
    last_active_at: u64,
    #[serde(alias = "session_name")]
    session_name: String,
    #[serde(alias = "last_turn")]
    last_turn: Option<SessionLastTurn>,
    #[serde(alias = "needs_user_attention")]
    needs_user_attention: Option<String>,
}

#[derive(Clone, Copy)]
enum Admission<'a> {
    Preview,
    Confirmed(&'a Candidate),
    RemovedSession,
}

impl History {
    fn candidate(
        &self,
        sessions: &Path,
        period: Period,
        unfinished: bool,
        now: u64,
        admission: Admission,
    ) -> Option<(Candidate, Option<SessionWriteLock>)> {
        let id = &self.session_id;
        let dir = sessions.join(id);
        checked_path(sessions, &dir).ok()?;
        // A missing sessions root, inaccessible directory or link cannot prove deletion.
        if !fs::symlink_metadata(sessions).ok()?.is_dir() {
            return None;
        }
        let lock = if !matches!(admission, Admission::Preview) {
            Some(SessionWriteLock::try_acquire_for_maintenance(sessions, id).ok()?)
        } else {
            if SessionWriteLock::is_in_use(sessions, id).ok()? {
                return None;
            }
            None
        };
        let candidate = match fs::symlink_metadata(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                removed_candidate(id, self.last_updated)?
            }
            Ok(metadata)
                if metadata.is_dir()
                    && !unfinished
                    && !matches!(admission, Admission::RemovedSession) =>
            {
                self.live_candidate(&dir, period, now)?
            }
            _ => return None,
        };
        if matches!(admission, Admission::Confirmed(expected) if expected != &candidate) {
            return None;
        }
        Some((candidate, lock))
    }

    fn live_candidate(&self, dir: &Path, period: Period, now: u64) -> Option<Candidate> {
        if !matches!(fs::symlink_metadata(dir.join("session-revert.json")), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        {
            return None;
        }
        let activity: Activity = json(&dir.join("metadata.json")).ok()?;
        if activity.schema_version > SESSION_STORAGE_SCHEMA_VERSION
            || activity.session_id != self.session_id
            || activity.last_active_at == 0
            || activity.needs_user_attention.is_some()
            || activity.last_turn.as_ref().is_some_and(|t| {
                t.status == TurnStatus::InProgress || t.recovery_pending == Some(true)
            })
        {
            return None;
        }
        let updated = self.last_updated.duration_since(UNIX_EPOCH).ok()?;
        let active_at = activity.last_active_at.max(millis(self.last_updated)?);
        if !period.includes(active_at, now) {
            return None;
        }
        Some(Candidate {
            removed: false,
            session_id: self.session_id.clone(),
            name: activity.session_name,
            active_at,
            revision: format!("{}:{}", activity.last_active_at, updated.as_nanos()),
        })
    }
}

fn removed_candidate(id: &str, updated: SystemTime) -> Option<Candidate> {
    Some(Candidate {
        session_id: id.to_owned(),
        name: String::new(),
        active_at: millis(updated)?,
        revision: format!(
            "removed:{}",
            updated.duration_since(UNIX_EPOCH).ok()?.as_nanos()
        ),
        removed: true,
    })
}

pub(super) fn json<T: DeserializeOwned>(path: &Path) -> SnapshotResult<T> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > CONTENT_LIMIT as u64 {
        return Err(invalid("Invalid storage record"));
    }
    let bytes = read_bounded(File::open(path)?, CONTENT_LIMIT, path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

struct StoredFile {
    path: PathBuf,
    bytes: u64,
}
impl StoredFile {
    fn new(entry: &fs::DirEntry) -> SnapshotResult<Self> {
        Ok(Self {
            path: entry.path(),
            bytes: entry.metadata()?.len(),
        })
    }
}

#[derive(Default)]
pub(super) struct Plan {
    pub(super) candidates: BTreeMap<String, Candidate>,
    pub(super) single_session_bytes: u64,
    pub(super) removed_estimated_bytes: u64,
    removed_histories: BTreeMap<String, StoredFile>,
    pub(super) skipped: usize,
    pub(super) unverified_references: bool,
    histories: BTreeMap<String, StoredFile>,
    handles: BTreeMap<String, StoredFile>,
    blobs: BTreeMap<String, StoredFile>,
    // Held across the final revalidation, unlink and in-memory invalidation.
    locks: Vec<SessionWriteLock>,
}

impl Plan {
    pub(super) fn load(
        scope: &Scope,
        period: Period,
        confirmed: Option<&[Candidate]>,
    ) -> SnapshotResult<Self> {
        let selected = confirmed.map(|items| {
            items
                .iter()
                .map(|c| (c.session_id.as_str(), Admission::Confirmed(c)))
                .collect()
        });
        Self::load_selected(scope, period, selected)
    }

    pub(super) fn for_deleted_sessions(scope: &Scope, ids: &[String]) -> SnapshotResult<Self> {
        let selected = ids
            .iter()
            .map(|id| (id.as_str(), Admission::RemovedSession))
            .collect();
        Self::load_selected(scope, Period::All, Some(selected))
    }

    fn load_selected(
        scope: &Scope,
        period: Period,
        selected: Option<HashMap<&str, Admission>>,
    ) -> SnapshotResult<Self> {
        let context = &scope.context;
        let now = millis(SystemTime::now()).ok_or_else(|| invalid("Invalid system clock"))?;
        let mut plan = Self::default();
        let mut owners: HashMap<String, HashSet<String>> = HashMap::new();
        let mut unknown_owners = false;
        for entry in entries(&context.snapshot_operations_dir)? {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let history: History = match json::<History>(&path) {
                Ok(h)
                    if openbitfun_core_types::validate_session_id(&h.session_id).is_ok()
                        && path.file_stem().and_then(|s| s.to_str())
                            == Some(&sanitize_id(&h.session_id)) =>
                {
                    h
                }
                _ => {
                    unknown_owners = true;
                    plan.skipped += 1;
                    continue;
                }
            };
            let history_file = StoredFile::new(&entry)?;
            plan.single_session_bytes += history_file.bytes;
            let mut unfinished = false;
            for op in history.turns.values().flat_map(|t| &t.operations) {
                unfinished |= op.completed == Some(false);
                for id in [
                    op.before_snapshot_id.as_ref(),
                    op.after_snapshot_id.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    owners
                        .entry(id.clone())
                        .or_default()
                        .insert(history.session_id.clone());
                }
            }
            // Acquire a fresh lock before reading activity. Loaded writers (including
            // this process), stale pending operations and outstanding reverts are retained.
            let id = &history.session_id;
            let admission = match &selected {
                Some(items) => items.get(id.as_str()).copied(),
                None => Some(Admission::Preview),
            };
            let candidate = admission
                .and_then(|a| history.candidate(&scope.sessions, period, unfinished, now, a));
            if let Some((candidate, lock)) = candidate {
                if let Some(lock) = lock {
                    plan.locks.push(lock);
                }
                if candidate.removed {
                    plan.removed_estimated_bytes += history_file.bytes;
                    plan.removed_histories.insert(id.clone(), history_file);
                } else {
                    plan.histories.insert(id.clone(), history_file);
                }
                plan.candidates.insert(id.clone(), candidate);
            } else {
                plan.skipped += 1;
            }
        }

        // Older reverts could borrow operation handles, even from another
        // session. Include markers belonging to sessions without an operation file.
        let mut checkpoints = HashSet::new();
        for entry in entries(&scope.sessions)? {
            if entry.file_type()?.is_symlink() {
                unknown_owners = true;
                continue;
            }
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path().join("session-revert.json");
            if matches!(fs::symlink_metadata(&path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
            {
                continue;
            }
            match json::<crate::agentic::session::revert::SessionRevertState>(&path) {
                Ok(state)
                    if state.schema_version
                        == crate::agentic::session::revert::SESSION_REVERT_SCHEMA_VERSION =>
                {
                    checkpoints.extend(
                        state
                            .workspace_checkpoint
                            .into_iter()
                            .filter_map(|c| c.snapshot_id),
                    );
                }
                _ => unknown_owners = true,
            }
        }

        plan.collect_files(context, &owners, &checkpoints, unknown_owners)?;
        Ok(plan)
    }

    fn collect_files(
        &mut self,
        context: &WorkspaceRuntimeContext,
        owners: &HashMap<String, HashSet<String>>,
        checkpoints: &HashSet<String>,
        unknown_owners: bool,
    ) -> SnapshotResult<()> {
        let mut kept_hashes = HashSet::new();
        let mut owned_hashes = HashSet::new();
        let mut hash_owners: HashMap<String, HashSet<String>> = HashMap::new();
        let mut shared_hashes = HashSet::new();
        let mut unknown_hash = false;
        for (dir, baseline) in [
            (&context.snapshot_baselines_dir, true),
            (&context.snapshot_metadata_dir, false),
        ] {
            for entry in entries(dir)? {
                let path = entry.path();
                if path.extension().is_none_or(|ext| ext != "json") {
                    continue;
                }
                if !entry.file_type()?.is_file() {
                    unknown_hash = true;
                    continue;
                }
                let stored = StoredFile::new(&entry)?;
                let snapshot = match descriptor(&path) {
                    Ok(s) if path.file_stem().and_then(|v| v.to_str()) == Some(&s.snapshot_id) => s,
                    _ => {
                        unknown_hash = true;
                        continue;
                    }
                };
                let id = snapshot.snapshot_id;
                let hash = snapshot.content_hash;
                let refs = owners.get(&id);
                let protected = baseline
                    || snapshot.snapshot_type == super::super::SnapshotType::Baseline
                    || id.starts_with("baseline_")
                    || checkpoints.contains(&id)
                    || refs.is_none();
                if !unknown_owners && !protected && refs.is_some_and(|r| r.len() == 1) {
                    self.single_session_bytes += stored.bytes;
                }
                if protected {
                    shared_hashes.insert(hash.clone());
                }
                if let Some(refs) = refs {
                    hash_owners
                        .entry(hash.clone())
                        .or_default()
                        .extend(refs.iter().cloned());
                }
                if !unknown_owners
                    && !protected
                    && refs.is_some_and(|r| r.iter().all(|s| self.candidates.contains_key(s)))
                {
                    owned_hashes.insert(hash);
                    if refs
                        .is_some_and(|r| r.iter().all(|s| self.removed_histories.contains_key(s)))
                    {
                        self.removed_estimated_bytes += stored.bytes;
                    }
                    self.handles.insert(id, stored);
                } else {
                    kept_hashes.insert(hash);
                }
            }
        }
        for entry in entries(&context.snapshot_by_hash_dir)? {
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            let Some(hash) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|_| path.extension().is_some_and(|e| e == "snap"))
            else {
                continue;
            };
            let stored = StoredFile::new(&entry)?;
            if !unknown_hash
                && !unknown_owners
                && !shared_hashes.contains(hash)
                && hash_owners.get(hash).is_some_and(|r| r.len() == 1)
            {
                self.single_session_bytes += stored.bytes;
            }
            if !unknown_hash && owned_hashes.contains(hash) && !kept_hashes.contains(hash) {
                if hash_owners
                    .get(hash)
                    .is_some_and(|r| r.iter().all(|s| self.removed_histories.contains_key(s)))
                {
                    self.removed_estimated_bytes += stored.bytes;
                }
                self.blobs.insert(hash.to_owned(), stored);
            }
        }
        self.unverified_references = unknown_owners || unknown_hash;
        if self.unverified_references && !self.removed_histories.is_empty() {
            // Keep the ownership chain intact until every reference can be checked.
            self.skipped += self.removed_histories.len();
            self.candidates.retain(|_, candidate| !candidate.removed);
            self.removed_histories.clear();
            self.removed_estimated_bytes = 0;
            self.handles
                .retain(|id, _| owners[id].iter().all(|s| self.candidates.contains_key(s)));
        }
        Ok(())
    }

    pub(super) fn estimate(&self) -> u64 {
        self.histories
            .values()
            .chain(self.removed_histories.values())
            .chain(self.handles.values())
            .chain(self.blobs.values())
            .map(|f| f.bytes)
            .sum()
    }

    pub(super) fn remove(&mut self, requested: usize) -> (CleanupResult, Removed) {
        let mut result = CleanupResult::default();
        let mut removed = Removed::default();
        // Detach live undo history first. For removed sessions the history is the
        // retry manifest: retain it and its descriptors until exclusive blobs are gone.
        // After a crash the next scan can reconstruct the remaining work from disk.
        unlink(&self.histories, &mut removed.sessions, &mut result);
        if self.removed_histories.is_empty() {
            unlink(&self.handles, &mut removed.handles, &mut result);
            unlink(&self.blobs, &mut removed.blobs, &mut result);
        } else {
            unlink(&self.blobs, &mut removed.blobs, &mut result);
            unlink(&self.handles, &mut removed.handles, &mut result);
            unlink(&self.removed_histories, &mut removed.sessions, &mut result);
        }
        result.removed_sessions = removed.sessions.len();
        result.skipped_sessions = requested.saturating_sub(result.removed_sessions);
        (result, removed)
    }
}

fn unlink(
    files: &BTreeMap<String, StoredFile>,
    ids: &mut HashSet<String>,
    result: &mut CleanupResult,
) {
    if !result.errors.is_empty() {
        return;
    }
    for (id, file) in files {
        match fs::remove_file(&file.path) {
            Ok(()) => {
                ids.insert(id.clone());
                result.freed_bytes += file.bytes;
            }
            Err(e) => result
                .errors
                .push(format!("{}: {}", file.path.display(), e)),
        }
    }
}
