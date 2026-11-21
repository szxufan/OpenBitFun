use super::support::{evidence_event_ids, test_manager, test_path_manager, TestWorkspace};
use crate::agentic::core::SessionConfig;
use crate::agentic::persistence::{PersistenceManager, SessionBranchRequest};
use crate::agentic::session::{EvidenceLedgerCheckpoint, PersistedEvidenceLedgerFile};
#[cfg(feature = "remote-workspace")]
use crate::agentic::session::{EvidenceLedgerEventStatus, EvidenceLedgerTargetKind};
use crate::service::session::{DialogTurnData, UserMessageData};
use openbitfun_services_core::session::SessionBranchBoundary;
use std::collections::HashSet;
use std::sync::Arc;

#[tokio::test]
async fn records_subagent_partial_timeout_in_evidence_ledger() {
    let persistence_manager =
        Arc::new(PersistenceManager::new(test_path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);

    let event = manager
        .record_subagent_partial_timeout(
            "session-a",
            "turn-a",
            "ReviewSecurity",
            "Found token logging before timeout.",
            Some("timeout"),
        )
        .await
        .expect("in-memory evidence should record");

    assert!(!event.event_id.is_empty());
    let events = manager.evidence_events_for_turn("session-a", "turn-a");
    assert_eq!(events, vec![event.clone()]);
    let summary = manager.evidence_summary_for_session("session-a", 10);
    assert_eq!(summary.partial_subagent_results.len(), 1);
    assert_eq!(summary.partial_subagent_results[0].event_id, event.event_id);
}

#[tokio::test]
async fn evidence_ledger_persists_across_session_unload_and_restore() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Durable evidence".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let storage_path = manager
        .effective_session_storage_path(&session.session_id)
        .await
        .expect("storage path");
    let turn = DialogTurnData::new(
        "turn-a".to_string(),
        0,
        session.session_id.clone(),
        UserMessageData {
            id: "turn-a-user".to_string(),
            content: "continue".to_string(),
            timestamp: 1,
            metadata: None,
        },
    );
    persistence_manager
        .save_dialog_turn(workspace.path(), &turn)
        .await
        .expect("turn should save");
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("session should be active")
        .dialog_turn_ids = vec!["turn-a".to_string()];
    let event = manager
        .record_checkpoint_created(
            &session.session_id,
            "turn-a",
            "Edit",
            "src/lib.rs",
            EvidenceLedgerCheckpoint {
                current_branch: Some("feature/evidence".to_string()),
                dirty_state_summary: "staged=0, unstaged=1, untracked=0".to_string(),
                touched_files: vec!["src/lib.rs".to_string()],
                diff_hash: Some("abc123".to_string()),
            },
        )
        .await
        .expect("checkpoint should persist before mutation");
    let ledger_path = storage_path
        .join(&session.session_id)
        .join("evidence-ledger.json");
    let stored: PersistedEvidenceLedgerFile =
        serde_json::from_slice(&std::fs::read(&ledger_path).expect("ledger sidecar should exist"))
            .expect("ledger sidecar should deserialize");
    assert_eq!(stored.session_id, session.session_id);
    assert_eq!(stored.events, vec![event.clone()]);

    assert!(manager
        .unload_session_from_memory(&session.session_id)
        .await
        .expect("session should unload"));
    assert!(manager
        .evidence_events_for_turn(&session.session_id, "turn-a")
        .is_empty());

    manager
        .restore_session_from_storage_path(&storage_path, &session.session_id)
        .await
        .expect("session should restore with evidence");
    assert_eq!(
        manager.evidence_events_for_turn(&session.session_id, "turn-a"),
        vec![event]
    );
    let summary = manager.evidence_summary_for_session(&session.session_id, 10);
    assert_eq!(summary.latest_checkpoints.len(), 1);
    assert_eq!(summary.latest_checkpoints[0].target, "src/lib.rs");
}

#[tokio::test]
async fn evidence_ledger_write_failure_does_not_publish_memory_only_evidence() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Evidence write failure".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    persistence_manager.fail_next_evidence_ledger_write_for_test(&session.session_id);

    manager
        .record_subagent_partial_timeout(
            &session.session_id,
            "turn-a",
            "ReviewSecurity",
            "Partial result",
            Some("timeout"),
        )
        .await
        .expect_err("durable append failure must be visible");

    assert!(manager
        .evidence_events_for_turn(&session.session_id, "turn-a")
        .is_empty());
    assert!(manager
        .evidence_summary_for_session(&session.session_id, 10)
        .partial_subagent_results
        .is_empty());
}

#[tokio::test]
async fn concurrent_evidence_appends_keep_disk_and_memory_complete() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = Arc::new(test_manager(persistence_manager));
    let session = manager
        .create_session(
            "Concurrent evidence".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let storage_path = manager
        .effective_session_storage_path(&session.session_id)
        .await
        .expect("storage path");

    let first_manager = manager.clone();
    let first_session_id = session.session_id.clone();
    let first = tokio::spawn(async move {
        first_manager
            .record_subagent_partial_timeout(
                &first_session_id,
                "turn-a",
                "ReviewSecurity",
                "First partial result",
                Some("timeout"),
            )
            .await
            .expect("first evidence append")
    });
    let second_manager = manager.clone();
    let second_session_id = session.session_id.clone();
    let second = tokio::spawn(async move {
        second_manager
            .record_subagent_partial_timeout(
                &second_session_id,
                "turn-b",
                "ReviewTests",
                "Second partial result",
                Some("timeout"),
            )
            .await
            .expect("second evidence append")
    });
    let first = first.await.expect("first append task");
    let second = second.await.expect("second append task");

    let ledger_path = storage_path
        .join(&session.session_id)
        .join("evidence-ledger.json");
    let stored: PersistedEvidenceLedgerFile =
        serde_json::from_slice(&std::fs::read(ledger_path).expect("ledger sidecar should exist"))
            .expect("ledger sidecar should deserialize");
    let mut stored_ids = stored
        .events
        .into_iter()
        .map(|event| event.event_id)
        .collect::<Vec<_>>();
    let mut memory_ids = manager
        .evidence_ledger
        .events_for_session(&session.session_id)
        .into_iter()
        .map(|event| event.event_id)
        .collect::<Vec<_>>();
    let mut expected_ids = vec![first.event_id, second.event_id];
    stored_ids.sort();
    memory_ids.sort();
    expected_ids.sort();

    assert_eq!(stored_ids, expected_ids);
    assert_eq!(memory_ids, expected_ids);
}

#[tokio::test]
async fn corrupt_evidence_ledger_blocks_restore_without_overwriting_original_bytes() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Corrupt evidence".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let storage_path = manager
        .effective_session_storage_path(&session.session_id)
        .await
        .expect("storage path");
    let ledger_path = storage_path
        .join(&session.session_id)
        .join("evidence-ledger.json");
    let corrupt_bytes = b"{not valid evidence";
    std::fs::write(&ledger_path, corrupt_bytes).expect("corrupt fixture should write");
    assert!(manager
        .unload_session_from_memory(&session.session_id)
        .await
        .expect("session should unload"));

    manager
        .restore_session_from_storage_path(&storage_path, &session.session_id)
        .await
        .expect_err("corrupt evidence must not degrade to an empty ledger");

    assert!(manager.get_session(&session.session_id).is_none());
    assert_eq!(
        std::fs::read(&ledger_path).expect("corrupt sidecar should remain"),
        corrupt_bytes
    );
}

#[tokio::test]
async fn corrupt_evidence_ledger_blocks_retention_without_overwriting_original_bytes() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Corrupt retention evidence".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let ledger_path = manager
        .effective_session_storage_path(&session.session_id)
        .await
        .expect("storage path")
        .join(&session.session_id)
        .join("evidence-ledger.json");
    let corrupt_bytes = b"{not valid evidence";
    std::fs::write(&ledger_path, corrupt_bytes).expect("corrupt fixture should write");

    persistence_manager
        .retain_evidence_ledger_events(workspace.path(), &session.session_id, &HashSet::new())
        .await
        .expect_err("corrupt evidence must not degrade to an empty ledger");

    assert_eq!(
        std::fs::read(&ledger_path).expect("corrupt sidecar should remain"),
        corrupt_bytes
    );
}

#[cfg(feature = "remote-workspace")]
#[tokio::test]
async fn remote_workspace_evidence_uses_the_resolved_session_mirror() {
    crate::service::workspace::legacy_compat::register_remote_fixture(
        "/home/wsp/project",
        "ssh-1",
        "dev-host",
    )
    .await;
    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let persistence_manager =
        Arc::new(PersistenceManager::new(path_manager.clone()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Remote evidence".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some("/home/wsp/project".to_string()),
                remote_connection_id: Some("ssh-1".to_string()),
                remote_ssh_host: Some("dev-host".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("remote session should create");
    let event = manager
        .record_subagent_partial_timeout(
            &session.session_id,
            "turn-a",
            "ReviewSecurity",
            "Remote partial result",
            Some("timeout"),
        )
        .await
        .expect("remote evidence should persist");
    let sessions_dir = crate::service::WorkspaceRuntimeService::new(path_manager)
        .context_for_remote_workspace("dev-host", "/home/wsp/project")
        .sessions_dir;
    let ledger_path = sessions_dir
        .join(&session.session_id)
        .join("evidence-ledger.json");
    let stored: PersistedEvidenceLedgerFile = serde_json::from_slice(
        &std::fs::read(ledger_path).expect("remote ledger sidecar should exist"),
    )
    .expect("remote ledger sidecar should deserialize");

    assert_eq!(stored.events, vec![event]);
    assert_eq!(
        stored.events[0].target_kind,
        EvidenceLedgerTargetKind::Subagent
    );
    assert_eq!(
        stored.events[0].status,
        EvidenceLedgerEventStatus::PartialTimeout
    );
}

#[tokio::test]
async fn restore_converges_evidence_sidecar_to_surviving_turns() {
    // P2 regression: after restore, the sidecar must be converged to the
    // surviving turns so a subsequent evidence append does not resurrect
    // stale events from a turn that no longer exists.
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Converge evidence".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let storage_path = manager
        .effective_session_storage_path(&session.session_id)
        .await
        .expect("storage path");

    // Create two turns with evidence.
    for index in 0..2 {
        let turn = DialogTurnData::new(
            format!("turn-{index}"),
            index,
            session.session_id.clone(),
            UserMessageData {
                id: format!("turn-{index}-user"),
                content: format!("prompt {index}"),
                timestamp: index as u64,
                metadata: None,
            },
        );
        persistence_manager
            .save_dialog_turn(workspace.path(), &turn)
            .await
            .expect("turn should save");
        let messages = (0..=index)
            .map(|i| crate::agentic::core::Message::user(format!("prompt {i}")))
            .collect::<Vec<_>>();
        persistence_manager
            .save_turn_context_snapshot(workspace.path(), &session.session_id, index, &messages)
            .await
            .expect("snapshot should save");
    }
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("session should be active")
        .dialog_turn_ids = vec!["turn-0".to_string(), "turn-1".to_string()];
    for index in 0..2 {
        manager
            .record_subagent_partial_timeout(
                &session.session_id,
                &format!("turn-{index}"),
                "ReviewSecurity",
                &format!("Partial turn {index}"),
                Some("timeout"),
            )
            .await
            .expect("evidence should persist");
    }
    let ledger_path = storage_path
        .join(&session.session_id)
        .join("evidence-ledger.json");
    assert_eq!(evidence_event_ids(&ledger_path).len(), 2);

    // Simulate an older build removing turn-1 from history but leaving
    // the evidence sidecar untouched.
    persistence_manager
        .delete_dialog_turns_from(workspace.path(), &session.session_id, 1)
        .await
        .expect("turn-1 should be deleted");
    persistence_manager
        .delete_turn_context_snapshots_from(workspace.path(), &session.session_id, 1)
        .await
        .expect("snapshot-1 should be deleted");

    // Unload and restore. The restore should converge the sidecar.
    assert!(manager
        .unload_session_from_memory(&session.session_id)
        .await
        .expect("session should unload"));
    manager
        .restore_session_from_storage_path(&storage_path, &session.session_id)
        .await
        .expect("session should restore");

    // The sidecar should now only contain turn-0's evidence.
    let sidecar_ids = evidence_event_ids(&ledger_path);
    assert_eq!(sidecar_ids.len(), 1);
    assert!(manager
        .evidence_events_for_turn(&session.session_id, "turn-1")
        .is_empty());

    // Appending new evidence must not resurrect turn-1's event.
    manager
        .record_subagent_partial_timeout(
            &session.session_id,
            "turn-0",
            "ReviewLogic",
            "New partial result",
            Some("timeout"),
        )
        .await
        .expect("new evidence should persist");
    let final_ids = evidence_event_ids(&ledger_path);
    assert_eq!(final_ids.len(), 2);
    assert!(manager
        .evidence_events_for_turn(&session.session_id, "turn-1")
        .is_empty());
    let summary = manager.evidence_summary_for_session(&session.session_id, 10);
    assert_eq!(summary.partial_subagent_results.len(), 2);
}

#[tokio::test]
async fn branch_session_copies_evidence_ledger_for_inherited_turns() {
    // P1 regression: forking a session must copy the evidence ledger,
    // filtered to the copied turns and rewritten to the target session.
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Fork evidence".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let storage_path = manager
        .effective_session_storage_path(&session.session_id)
        .await
        .expect("storage path");

    // Create two turns with evidence.
    for index in 0..2 {
        let turn = DialogTurnData::new(
            format!("turn-{index}"),
            index,
            session.session_id.clone(),
            UserMessageData {
                id: format!("turn-{index}-user"),
                content: format!("prompt {index}"),
                timestamp: index as u64,
                metadata: None,
            },
        );
        persistence_manager
            .save_dialog_turn(workspace.path(), &turn)
            .await
            .expect("turn should save");
        let messages = (0..=index)
            .map(|i| crate::agentic::core::Message::user(format!("prompt {i}")))
            .collect::<Vec<_>>();
        persistence_manager
            .save_turn_context_snapshot(workspace.path(), &session.session_id, index, &messages)
            .await
            .expect("snapshot should save");
    }
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("session should be active")
        .dialog_turn_ids = vec!["turn-0".to_string(), "turn-1".to_string()];
    let turn_0_event = manager
        .record_checkpoint_created(
            &session.session_id,
            "turn-0",
            "Edit",
            "src/lib.rs",
            EvidenceLedgerCheckpoint {
                current_branch: Some("feature/evidence".to_string()),
                dirty_state_summary: "staged=0".to_string(),
                touched_files: vec!["src/lib.rs".to_string()],
                diff_hash: Some("abc".to_string()),
            },
        )
        .await
        .expect("turn-0 checkpoint should persist");
    let turn_1_event = manager
        .record_subagent_partial_timeout(
            &session.session_id,
            "turn-1",
            "ReviewSecurity",
            "Partial turn 1",
            Some("timeout"),
        )
        .await
        .expect("turn-1 evidence should persist");

    // Branch through turn-0 only.
    let branch_result = persistence_manager
        .branch_session(
            workspace.path(),
            &SessionBranchRequest {
                source_session_id: session.session_id.clone(),
                source_turn_id: "turn-0".to_string(),
                boundary: SessionBranchBoundary::ThroughTurn,
            },
        )
        .await
        .expect("branch should succeed");

    // The fork should have turn-0's evidence but not turn-1's.
    let fork_ledger_path = storage_path
        .join(&branch_result.session_id)
        .join("evidence-ledger.json");
    assert!(
        fork_ledger_path.exists(),
        "fork evidence sidecar should exist"
    );
    let fork_stored: PersistedEvidenceLedgerFile =
        serde_json::from_slice(&std::fs::read(&fork_ledger_path).expect("fork ledger should read"))
            .expect("fork ledger should deserialize");
    assert_eq!(fork_stored.session_id, branch_result.session_id);
    assert_eq!(fork_stored.events.len(), 1);
    assert_eq!(fork_stored.events[0].turn_id, "turn-0");
    assert_eq!(fork_stored.events[0].session_id, branch_result.session_id);
    assert_eq!(fork_stored.events[0].event_id, turn_0_event.event_id);
    // The checkpoint summary should be preserved.
    assert!(fork_stored.events[0].checkpoint.is_some());
    // turn-1's evidence must not be in the fork.
    assert!(fork_stored
        .events
        .iter()
        .all(|e| e.event_id != turn_1_event.event_id));
}
