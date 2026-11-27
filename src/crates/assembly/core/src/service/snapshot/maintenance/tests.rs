use super::*;
use crate::service::workspace_runtime::WorkspaceRuntimeTarget;
use openbitfun_services_core::session::SessionWriteLock;
use serde_json::json;
use std::time::Duration;

fn scope(root: &Path) -> Scope {
    let context = WorkspaceRuntimeContext::new(
        WorkspaceRuntimeTarget::LocalWorkspace {
            workspace_root: root.join("worktree"),
        },
        root.join("project"),
    );
    for dir in context.required_directories() {
        fs::create_dir_all(dir).unwrap();
    }
    Scope {
        id: "project".into(),
        name: "Project".into(),
        location: "local".into(),
        remote: false,
        sessions: context.sessions_dir.clone(),
        context,
    }
}

fn history(scope: &Scope, id: &str, days: u64, snapshots: &[&str]) {
    let time = SystemTime::now() - Duration::from_secs(86_400 * days);
    let dir = scope.sessions.join(id);
    fs::create_dir_all(&dir).unwrap();
    // Legacy snake-case metadata and operations without an explicit completed flag.
    fs::write(
        dir.join("metadata.json"),
        json!({
            "session_id": id, "session_name": id, "last_active_at": millis(time)
        })
        .to_string(),
    )
    .unwrap();
    let operations = snapshots
        .iter()
        .map(|s| json!({"before_snapshot_id":s}))
        .collect::<Vec<_>>();
    fs::write(
        scope
            .context
            .snapshot_operations_dir
            .join(format!("{id}.json")),
        json!({"session_id":id,"last_updated":time,"turns":{"0":{"operations":operations}}})
            .to_string(),
    )
    .unwrap();
}

fn handle(scope: &Scope, id: &str, hash: &str, baseline: bool) -> PathBuf {
    let dir = if baseline {
        &scope.context.snapshot_baselines_dir
    } else {
        &scope.context.snapshot_metadata_dir
    };
    let path = dir.join(format!("{id}.json"));
    fs::write(&path,json!({"snapshot_id":id,"file_path":"a","content_hash":hash,
        "snapshot_type":if baseline {"Baseline"} else {"Before"}, "compressed_content":[1,2,3],
        "timestamp":UNIX_EPOCH,"metadata":{"size":3,"permissions":null,"last_modified":UNIX_EPOCH,"encoding":"binary"}}).to_string()).unwrap();
    path
}

#[test]
fn cleanup_preserves_shared_content_checkpoints_and_session_records() {
    let tmp = tempfile::tempdir().unwrap();
    let scope = scope(tmp.path());
    let hash_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let hash_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let hash_c = "cccccccccccccccccccccccccccccccc";
    history(
        &scope,
        "old",
        40,
        &["own", "with-base", "shared-id", "revert-id"],
    );
    history(&scope, "recent", 1, &["shared-id"]);
    let own = handle(&scope, "own", hash_a, false);
    let with_base = handle(&scope, "with-base", hash_b, false);
    let baseline = handle(&scope, "baseline_x", hash_b, true);
    let shared = handle(&scope, "shared-id", hash_c, false);
    let borrowed = handle(&scope, "revert-id", hash_c, false);
    let marker = scope.sessions.join("undo");
    fs::create_dir_all(&marker).unwrap();
    fs::write(
        marker.join("session-revert.json"),
        json!({"schemaVersion":1,"boundaryTurn":0,"originalTurnEnd":1,
        "phase":"staged","workspaceCheckpoint":[{"path":"a","snapshotId":"revert-id"}]})
        .to_string(),
    )
    .unwrap();
    for hash in [hash_a, hash_b, hash_c] {
        fs::write(
            scope
                .context
                .snapshot_by_hash_dir
                .join(format!("{hash}.snap")),
            b"data",
        )
        .unwrap();
    }
    let large = scope
        .context
        .snapshot_by_hash_dir
        .join(format!("{hash_a}.snap"));
    // A sparse 41 GiB payload must be counted and deleted without reading it.
    #[cfg(unix)]
    fs::OpenOptions::new()
        .write(true)
        .open(&large)
        .unwrap()
        .set_len(41 << 30)
        .unwrap();
    let preview = plan::Plan::load(&scope, Period::ThirtyDays, None).unwrap();
    assert_eq!(preview.candidates.len(), 1);
    let confirmed = vec![preview.candidates["old"].clone()];
    let before = tree_bytes(&scope.context.snapshots_dir, false).unwrap();
    let mut cleanup = plan::Plan::load(&scope, Period::ThirtyDays, Some(&confirmed)).unwrap();
    assert!(SessionWriteLock::try_acquire_for_operation(&scope.sessions, "old").is_err());
    let (result, _) = cleanup.remove(1);
    assert_eq!(result.removed_sessions, 1);
    assert_eq!(result.freed_bytes, preview.estimate());
    assert_eq!(
        before - tree_bytes(&scope.context.snapshots_dir, false).unwrap(),
        result.freed_bytes
    );
    assert!(!own.exists() && !with_base.exists() && !large.exists());
    assert!(baseline.exists() && shared.exists() && borrowed.exists());
    for hash in [hash_b, hash_c] {
        assert!(scope
            .context
            .snapshot_by_hash_dir
            .join(format!("{hash}.snap"))
            .is_file());
    }
    assert!(scope.sessions.join("old/metadata.json").exists());
    assert!(scope
        .context
        .snapshot_operations_dir
        .join("recent.json")
        .exists());
    drop(cleanup);
    let (retry, _) = plan::Plan::load(&scope, Period::ThirtyDays, Some(&confirmed))
        .unwrap()
        .remove(1);
    assert_eq!((retry.skipped_sessions, retry.freed_bytes), (1, 0));
}

#[test]
fn rechecks_both_activity_clocks_locks_approval_and_undo_even_for_all() {
    let tmp = tempfile::tempdir().unwrap();
    let scope = scope(tmp.path());
    for id in [
        "new-snapshot",
        "new-chat",
        "locked",
        "approval",
        "undo",
        "lost",
        "warm",
        "unchanged",
    ] {
        history(&scope, id, 40, &[]);
    }
    let initial = plan::Plan::load(&scope, Period::All, None)
        .unwrap()
        .candidates
        .into_values()
        .collect::<Vec<_>>();
    let _writer = SessionWriteLock::try_acquire(&scope.sessions, "locked").unwrap();
    let now = SystemTime::now();
    let update = |path: PathBuf, key: &str, value: serde_json::Value| {
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record[key] = value;
        fs::write(path, record.to_string()).unwrap();
    };
    update(
        scope
            .context
            .snapshot_operations_dir
            .join("new-snapshot.json"),
        "last_updated",
        json!(now),
    );
    update(
        scope.sessions.join("new-chat/metadata.json"),
        "last_active_at",
        json!(millis(now)),
    );
    update(
        scope.sessions.join("approval/metadata.json"),
        "needsUserAttention",
        json!("tool_confirm"),
    );
    fs::write(scope.sessions.join("undo/session-revert.json"), b"{").unwrap();
    fs::remove_file(scope.sessions.join("lost/metadata.json")).unwrap();
    history(&scope, "warm", 15, &[]);
    let seven = plan::Plan::load(&scope, Period::SevenDays, None).unwrap();
    assert_eq!(seven.candidates.len(), 2);
    let month = plan::Plan::load(&scope, Period::ThirtyDays, None).unwrap();
    assert_eq!(month.candidates.len(), 1);
    let mut final_plan = plan::Plan::load(&scope, Period::All, Some(&initial)).unwrap();
    assert_eq!(
        final_plan.candidates.keys().collect::<Vec<_>>(),
        vec!["unchanged"]
    );
    assert_eq!(final_plan.remove(initial.len()).0.skipped_sessions, 7);
}

#[test]
fn inventory_counts_unregistered_and_connection_scoped_stores_once() {
    let tmp = tempfile::tempdir().unwrap();
    let mut scope = scope(tmp.path());
    let base = scope.context.snapshots_dir.clone();
    fs::write(base.join("legacy"), b"12345").unwrap();
    let scoped = base.join("workspace-identities/profile-a");
    fs::create_dir_all(&scoped).unwrap();
    fs::write(scoped.join("data"), b"abc").unwrap();
    let other = base.join("workspace-identities/profile-b");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("data"), b"1234").unwrap();
    scope.context.snapshots_dir = scoped.clone();
    let overview = inspect_roots(
        tmp.path(),
        BTreeMap::from([(scoped, Some(scope))]),
        Period::All,
    )
    .unwrap();
    assert_eq!((overview.total_bytes, overview.unidentified_bytes), (12, 9));
    assert_eq!(overview.workspaces[0].bytes, 3);
}

#[test]
fn another_writer_prevents_reclamation_until_it_releases_its_index() {
    let tmp = tempfile::tempdir().unwrap();
    let cached = ViewStamp::capture(tmp.path());
    assert!(cached.current());
    let own = WriterLease::register(tmp.path()).unwrap();
    let other = WriterLease::register(tmp.path()).unwrap();
    assert!(writer_lease::exclusive(tmp.path(), Some(&own)).is_err());
    drop(other);
    let exclusive = writer_lease::exclusive(tmp.path(), Some(&own)).unwrap();
    assert!(writer_lease::exclusive(tmp.path(), Some(&own)).is_err());
    let during = ViewStamp::capture(tmp.path());
    assert!(!cached.current());
    drop(exclusive);
    assert!(!during.current() && !cached.current());
    assert!(ViewStamp::capture(tmp.path()).current());
    assert!(writer_lease::exclusive(tmp.path(), None).is_err());
}

#[cfg(unix)]
#[test]
fn a_linked_store_cannot_escape_or_disable_another_workspace() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    fs::write(external.path().join("keep"), b"unused").unwrap();
    let scope = scope(tmp.path());
    let linked = tmp.path().join("other/snapshots");
    fs::create_dir_all(linked.parent().unwrap()).unwrap();
    symlink(external.path(), &linked).unwrap();
    history(&scope, "valid", 40, &[]);
    let overview = inspect_roots(
        tmp.path(),
        BTreeMap::from([(scope.context.snapshots_dir.clone(), Some(scope))]),
        Period::All,
    )
    .unwrap();
    assert_eq!(overview.scan_errors.len(), 1);
    assert_eq!(overview.workspaces[0].candidates.len(), 1);
    assert_eq!(overview.unidentified_bytes, 0);
}

#[test]
fn legacy_mirrors_are_counted_read_only_without_counting_transcripts() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("ssh-server/nested/project");
    let history = project.join("snapshots/operations/old.json");
    fs::create_dir_all(history.parent().unwrap()).unwrap();
    fs::write(&history, b"history").unwrap();
    let chat = project.join("sessions/s/snapshots/dialog.json");
    fs::create_dir_all(chat.parent().unwrap()).unwrap();
    fs::write(&chat, b"session record").unwrap();
    let mut overview = StorageOverview::default();
    inspect_legacy_mirrors(tmp.path(), &mut overview);
    assert_eq!((overview.total_bytes, overview.unidentified_bytes), (7, 7));
    assert!(overview.workspaces.is_empty() && history.is_file() && chat.is_file());
}

#[test]
fn orphan_preview_requires_positive_workspace_ownership_and_rechecks_the_session_under_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let scope = scope(tmp.path());
    history(&scope, "gone", 1, &["own", "borrowed"]);
    history(&scope, "retained", 1, &["borrowed"]);
    let own = handle(&scope, "own", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", false);
    let borrowed = handle(
        &scope,
        "borrowed",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        false,
    );
    let dir = scope.sessions.join("gone");
    fs::remove_dir_all(&dir).unwrap();
    let preview = plan::Plan::load(&scope, Period::All, None).unwrap();
    let confirmed = vec![preview.candidates["gone"].clone()];
    assert!(confirmed[0].removed);
    assert!(preview.candidates.contains_key("retained"));
    let history_bytes = fs::metadata(scope.context.snapshot_operations_dir.join("gone.json"))
        .unwrap()
        .len();
    let reclaimable = history_bytes + own.metadata().unwrap().len();
    for period in [Period::SevenDays, Period::ThirtyDays] {
        let preview = plan::Plan::load(&scope, period, None).unwrap();
        assert_eq!(
            preview.candidates.into_values().collect::<Vec<_>>(),
            confirmed
        );
        assert_eq!(preview.removed_estimated_bytes, reclaimable);
        let rechecked = plan::Plan::load(&scope, period, Some(&confirmed)).unwrap();
        assert_eq!(rechecked.estimate(), reclaimable);
    }
    let locked = SessionWriteLock::try_acquire(&scope.sessions, "gone").unwrap();
    assert!(plan::Plan::load(&scope, Period::All, Some(&confirmed))
        .unwrap()
        .candidates
        .is_empty());
    drop(locked);
    // An incomplete restore is not proof of deletion, even if its metadata is missing.
    fs::create_dir(&dir).unwrap();
    assert!(plan::Plan::load(&scope, Period::All, Some(&confirmed))
        .unwrap()
        .candidates
        .is_empty());
    history(&scope, "gone", 40, &["own", "borrowed"]);
    assert!(plan::Plan::load(&scope, Period::All, Some(&confirmed))
        .unwrap()
        .candidates
        .is_empty());
    assert!(own.exists() && borrowed.exists());
    let legacy: Candidate = serde_json::from_value(json!({
        "sessionId":"active","name":"Active","activeAt":42,"revision":"42:1"
    }))
    .unwrap();
    assert!(!legacy.removed);
    assert_eq!(
        serde_json::from_value::<Candidate>(serde_json::to_value(&legacy).unwrap()).unwrap(),
        legacy
    );
}

#[tokio::test]
async fn pending_deletion_survives_restart_and_another_writer_then_reclaims_only_exclusive_files() {
    let tmp = tempfile::tempdir().unwrap();
    let projects = tmp.path().join("projects");
    let workspace = scope(&projects);
    let hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    history(&workspace, "gone", 1, &["own", "shared", "with-base"]);
    history(&workspace, "other", 1, &["shared"]);
    let own = handle(&workspace, "own", hash, false);
    let blob = workspace
        .context
        .snapshot_by_hash_dir
        .join(format!("{hash}.snap"));
    fs::write(&blob, b"old").unwrap();
    let base_hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let base = handle(&workspace, "baseline_file", base_hash, true);
    let with_base = handle(&workspace, "with-base", base_hash, false);
    let shared = handle(&workspace, "shared", base_hash, false);
    let shared_blob = workspace
        .context
        .snapshot_by_hash_dir
        .join(format!("{base_hash}.snap"));
    fs::write(&shared_blob, b"kept").unwrap();

    let deleting = SessionWriteLock::try_acquire(&workspace.sessions, "gone").unwrap();
    pending::PendingDeletion::prepare(&projects, &workspace, "gone")
        .unwrap()
        .unwrap();
    let replay = || {
        pending::pending_by_store(&projects)
            .unwrap()
            .into_values()
            .next()
            .unwrap()
    };
    // A second process replaying the intent before deletion commits cannot remove it.
    let early = clean_scope(
        scope(&projects),
        Cleanup::Pending(replay()),
        projects.clone(),
    )
    .await
    .unwrap();
    assert_eq!((early.removed_sessions, early.freed_bytes), (0, 0));
    assert!(own.exists());
    fs::remove_dir_all(workspace.sessions.join("gone")).unwrap();
    drop(deleting);
    let writer = WriterLease::register(&workspace.context.locks_dir).unwrap();
    assert!(clean_scope(
        scope(&projects),
        Cleanup::Pending(replay()),
        projects.clone()
    )
    .await
    .is_err());
    drop(writer);
    let expected = plan::Plan::load(&workspace, Period::All, None)
        .unwrap()
        .removed_estimated_bytes;
    let result = clean_scope(
        scope(&projects),
        Cleanup::Pending(replay()),
        projects.clone(),
    )
    .await
    .unwrap();
    assert!(result.errors.is_empty());
    assert_eq!((result.removed_sessions, result.freed_bytes), (1, expected));
    assert!(pending::pending_by_store(&projects).unwrap().is_empty());
    assert!(!own.exists() && !with_base.exists() && !blob.exists());
    assert!(base.exists() && shared.exists() && shared_blob.exists());
}

#[test]
fn orphan_removal_keeps_ownership_until_payload_and_descriptor_removal_complete() {
    let tmp = tempfile::tempdir().unwrap();
    let scope = scope(tmp.path());
    let hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    history(&scope, "gone", 1, &["own"]);
    fs::remove_dir_all(scope.sessions.join("gone")).unwrap();
    let own = handle(&scope, "own", hash, false);
    let blob = scope
        .context
        .snapshot_by_hash_dir
        .join(format!("{hash}.snap"));
    fs::write(&blob, b"payload").unwrap();
    let preview = plan::Plan::load(&scope, Period::ThirtyDays, None).unwrap();
    let confirmed = vec![preview.candidates["gone"].clone()];
    let mut plan = plan::Plan::load(&scope, Period::ThirtyDays, Some(&confirmed)).unwrap();
    let parked = tmp.path().join("parked");
    fs::rename(&own, &parked).unwrap(); // inject failure between unlink stages
    let (partial, _) = plan.remove(1);
    assert_eq!((partial.removed_sessions, partial.freed_bytes), (0, 7));
    assert!(!partial.errors.is_empty());
    drop(plan);
    fs::rename(&parked, &own).unwrap();
    let retry = plan::Plan::load(&scope, Period::ThirtyDays, Some(&confirmed))
        .unwrap()
        .remove(1)
        .0;
    assert_eq!(retry.removed_sessions, 1);
    assert!(retry.errors.is_empty());
    assert_eq!(partial.freed_bytes + retry.freed_bytes, preview.estimate());
}

#[test]
fn unverifiable_checkpoint_keeps_orphan_history_for_a_future_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let scope = scope(tmp.path());
    history(&scope, "gone", 1, &["own"]);
    let own = handle(&scope, "own", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", false);
    fs::remove_dir_all(scope.sessions.join("gone")).unwrap();
    let confirmed = plan::Plan::load(&scope, Period::All, None)
        .unwrap()
        .candidates
        .into_values()
        .collect::<Vec<_>>();
    fs::create_dir(scope.sessions.join("undo")).unwrap();
    fs::write(scope.sessions.join("undo/session-revert.json"), b"{").unwrap();
    let plan = plan::Plan::load(&scope, Period::All, Some(&confirmed)).unwrap();
    assert!(plan.unverified_references && plan.candidates.is_empty());
    assert_eq!((plan.estimate(), plan.removed_estimated_bytes), (0, 0));
    assert!(
        own.exists()
            && scope
                .context
                .snapshot_operations_dir
                .join("gone.json")
                .exists()
    );
}

#[test]
fn deletion_intents_stay_with_their_ssh_profile_after_a_workspace_rebind() {
    use openbitfun_services_core::workspace_identity::WorkspaceSessionIdentity;
    let tmp = tempfile::tempdir().unwrap();
    let projects = tmp.path().join("projects");
    let profile = |connection: &str| {
        let mut workspace = scope(&projects);
        let identity = WorkspaceSessionIdentity {
            workspace_kind: openbitfun_core_types::WorkspaceKind::Remote,
            hostname: "server".into(),
            logical_workspace_path: "/work".into(),
            remote_connection_id: Some(connection.into()),
        };
        let runtime = WorkspaceRuntimeContext::new(
            WorkspaceRuntimeTarget::RemoteWorkspaceMirror {
                ssh_host: identity.identity_host(),
                remote_root: "/work".into(),
            },
            projects.join("mirror"),
        );
        workspace.context = bound_snapshot_context(&identity, runtime).unwrap();
        workspace.remote = true;
        workspace.sessions = workspace.context.sessions_dir.clone();
        for dir in workspace.context.required_directories() {
            fs::create_dir_all(dir).unwrap();
        }
        history(&workspace, "same-id", 1, &[]);
        pending::PendingDeletion::prepare(&projects, &workspace, "same-id")
            .unwrap()
            .unwrap();
        workspace
    };
    let first = profile("profile-a");
    let second = profile("profile-b");
    assert_eq!(pending::pending_by_store(&projects).unwrap().len(), 2);
    fs::remove_file(first.context.snapshot_operations_dir.join("same-id.json")).unwrap();
    pending::acknowledge(&projects, &first, std::iter::once("same-id")).unwrap();
    assert_eq!(pending::pending_by_store(&projects).unwrap().len(), 1);
    assert!(second
        .context
        .snapshot_operations_dir
        .join("same-id.json")
        .is_file());
}
