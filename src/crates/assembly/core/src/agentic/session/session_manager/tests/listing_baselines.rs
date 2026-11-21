use super::super::SessionManager;
use super::support::{test_manager, TestWorkspace};
use crate::agentic::core::{Session, SessionConfig};
use crate::agentic::persistence::PersistenceManager;
use crate::agentic::skill_agent_snapshot::{SkillSnapshotEntry, TurnSkillAgentSnapshot};
use crate::service::session::{DialogTurnData, UserMessageData};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn latest_skill_agent_snapshot_scans_persistence_beyond_stale_cache_hit() {
    use crate::agentic::skill_agent_snapshot::{
        AgentSnapshotEntry, SkillSnapshotEntry, TurnSkillAgentSnapshot,
    };

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Skill agent snapshot".to_string(),
            "agent".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    manager
        .remember_turn_skill_agent_snapshot(
            &session.session_id,
            0,
            TurnSkillAgentSnapshot {
                skills: vec![SkillSnapshotEntry {
                    name: "skill-a".to_string(),
                    description: "desc-a".to_string(),
                    location: "/a".to_string(),
                }],
                subagents: vec![AgentSnapshotEntry {
                    id: "agent-a".to_string(),
                    description: "desc-a".to_string(),
                    default_tools: vec!["Read".to_string()],
                }],
                deferred_tools: None,
            },
        )
        .await;
    manager
        .remember_turn_skill_agent_snapshot(
            &session.session_id,
            1,
            TurnSkillAgentSnapshot {
                skills: vec![SkillSnapshotEntry {
                    name: "skill-a".to_string(),
                    description: "desc-a".to_string(),
                    location: "/a".to_string(),
                }],
                subagents: vec![AgentSnapshotEntry {
                    id: "agent-b".to_string(),
                    description: "desc-b".to_string(),
                    default_tools: vec!["Read".to_string(), "Grep".to_string()],
                }],
                deferred_tools: None,
            },
        )
        .await;

    manager
        .turn_skill_agent_snapshot_store
        .delete_session(&session.session_id);
    manager
        .turn_skill_agent_snapshot_store
        .create_session(&session.session_id);
    manager.turn_skill_agent_snapshot_store.set_snapshot(
        &session.session_id,
        0,
        TurnSkillAgentSnapshot {
            skills: vec![SkillSnapshotEntry {
                name: "skill-a".to_string(),
                description: "desc-a".to_string(),
                location: "/a".to_string(),
            }],
            subagents: vec![AgentSnapshotEntry {
                id: "agent-a".to_string(),
                description: "desc-a".to_string(),
                default_tools: vec!["Read".to_string()],
            }],
            deferred_tools: None,
        },
    );

    let latest = manager
        .latest_turn_skill_agent_snapshot_at_or_before(&session.session_id, 1)
        .await
        .expect("latest snapshot should exist");

    assert_eq!(latest.0, 1);
    assert_eq!(latest.1.subagents[0].id, "agent-b");
}

#[tokio::test]
async fn rebuild_skill_agent_listing_baseline_to_latest_removes_listing_diff_reminders() {
    use crate::agentic::core::{InternalReminderKind, Message, MessageSemanticKind};
    use crate::agentic::skill_agent_snapshot::{SkillSnapshotEntry, TurnSkillAgentSnapshot};

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Listing baseline rebuild".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    {
        let mut active = manager
            .sessions
            .get_mut(&session.session_id)
            .expect("session should be active");
        active.dialog_turn_ids = vec!["turn-0".to_string(), "turn-1".to_string()];
    }

    manager.context_store.replace_context(
        &session.session_id,
        vec![
            Message::internal_reminder(
                InternalReminderKind::SkillListingDiff,
                "# Skill Listing Update\n\nChanged",
            )
            .with_turn_id("turn-1".to_string()),
            Message::internal_reminder(
                InternalReminderKind::AgentListingDiff,
                "# Agent Listing Update\n\nChanged",
            )
            .with_turn_id("turn-1".to_string()),
            Message::user("real question".to_string())
                .with_turn_id("turn-1".to_string())
                .with_semantic_kind(MessageSemanticKind::ActualUserInput),
        ],
    );

    manager
        .remember_turn_skill_agent_snapshot(
            &session.session_id,
            0,
            TurnSkillAgentSnapshot {
                skills: vec![SkillSnapshotEntry {
                    name: "old-skill".to_string(),
                    description: "old".to_string(),
                    location: "/old".to_string(),
                }],
                ..Default::default()
            },
        )
        .await;
    manager
        .remember_turn_skill_agent_snapshot(
            &session.session_id,
            1,
            TurnSkillAgentSnapshot {
                skills: vec![SkillSnapshotEntry {
                    name: "new-skill".to_string(),
                    description: "new".to_string(),
                    location: "/new".to_string(),
                }],
                ..Default::default()
            },
        )
        .await;

    assert!(
        manager
            .rebuild_skill_agent_listing_baseline_to_latest(&session.session_id)
            .await
    );

    let context_messages = manager
        .context_store
        .get_context_messages(&session.session_id);
    assert_eq!(context_messages.len(), 1);
    assert_eq!(
        context_messages[0].metadata.semantic_kind,
        Some(MessageSemanticKind::ActualUserInput)
    );

    let baseline = manager
        .turn_skill_agent_snapshot(&session.session_id, 0)
        .await
        .expect("baseline snapshot should exist");
    assert_eq!(baseline.skills[0].name, "new-skill");
    assert!(manager
        .turn_skill_agent_snapshot(&session.session_id, 1)
        .await
        .is_none());

    let metadata = persistence_manager
        .load_session_metadata(workspace.path(), &session.session_id)
        .await
        .expect("metadata lookup should succeed")
        .expect("metadata should exist");
    assert_eq!(
        SessionManager::listing_baseline_rebuild_turn_index_from_metadata(Some(&metadata)),
        Some(1)
    );
}

#[tokio::test]
async fn listing_baseline_prefers_override_and_rebuild_carries_deferred_tools() {
    use crate::agentic::skill_agent_snapshot::{DeferredToolSnapshotEntry, TurnSkillAgentSnapshot};

    let deferred = |names: &[&str]| TurnSkillAgentSnapshot {
        deferred_tools: Some(
            names
                .iter()
                .map(|name| DeferredToolSnapshotEntry {
                    name: (*name).to_string(),
                    short_description: None,
                })
                .collect(),
        ),
        ..Default::default()
    };
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Deferred listing baseline".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let session_id = session.session_id.as_str();
    {
        let mut active = manager
            .sessions
            .get_mut(session_id)
            .expect("session should be active");
        active.dialog_turn_ids = vec!["turn-0".to_string(), "turn-1".to_string()];
    }

    manager
        .remember_turn_skill_agent_snapshot(session_id, 0, deferred(&["WebFetch"]))
        .await;
    let latest = deferred(&["WebFetch", "mcp__docs__search"]);
    manager
        .remember_turn_skill_agent_snapshot(session_id, 1, latest.clone())
        .await;
    assert_eq!(
        manager
            .skill_agent_listing_baseline_snapshot(session_id)
            .await,
        Some(deferred(&["WebFetch"]))
    );
    assert_eq!(
        persistence_manager
            .load_turn_skill_agent_snapshot(workspace.path(), session_id, 1)
            .await
            .expect("snapshot lookup should succeed"),
        Some(latest.clone())
    );

    manager
        .remember_skill_agent_baseline_override_snapshot(session_id, deferred(&["Cron"]))
        .await;
    assert_eq!(
        manager
            .skill_agent_listing_baseline_snapshot(session_id)
            .await,
        Some(deferred(&["Cron"]))
    );

    assert!(
        manager
            .rebuild_skill_agent_listing_baseline_to_latest(session_id)
            .await
    );
    assert_eq!(
        manager
            .skill_agent_listing_baseline_snapshot(session_id)
            .await,
        Some(latest.clone())
    );
    assert_eq!(
        manager.turn_skill_agent_snapshot(session_id, 0).await,
        Some(latest)
    );
}

#[tokio::test]
async fn restore_session_sanitizes_pre_cutoff_listing_diff_snapshot() {
    use crate::agentic::core::{InternalReminderKind, Message, MessageSemanticKind};

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session_id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        session_id.clone(),
        "Restore sanitize".to_string(),
        "Standard".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().to_string()),
            ..Default::default()
        },
    );
    session.dialog_turn_ids = vec!["turn-0".to_string(), "turn-1".to_string()];

    persistence_manager
        .save_session(workspace.path(), &session)
        .await
        .expect("session should save");

    let mut metadata = persistence_manager
        .load_session_metadata(workspace.path(), &session_id)
        .await
        .expect("metadata load should succeed")
        .expect("metadata should exist");
    metadata.custom_metadata = Some(json!({
        super::super::LISTING_BASELINE_REBUILD_TURN_INDEX_METADATA_KEY: 2,
    }));
    persistence_manager
        .save_session_metadata(workspace.path(), &metadata)
        .await
        .expect("metadata should save");

    for index in 0..=1 {
        let turn = DialogTurnData::new(
            format!("turn-{index}"),
            index,
            session_id.clone(),
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
    }

    persistence_manager
        .save_turn_context_snapshot(
            workspace.path(),
            &session_id,
            1,
            &[
                Message::internal_reminder(
                    InternalReminderKind::SkillListingDiff,
                    "# Skill Listing Update\n\nChanged",
                )
                .with_turn_id("turn-1".to_string()),
                Message::user("prompt 1".to_string())
                    .with_turn_id("turn-1".to_string())
                    .with_semantic_kind(MessageSemanticKind::ActualUserInput),
            ],
        )
        .await
        .expect("snapshot should save");

    let restored = manager
        .restore_session(workspace.path(), &session_id)
        .await
        .expect("session should restore");

    assert_eq!(
        restored.dialog_turn_ids,
        vec!["turn-0".to_string(), "turn-1".to_string()]
    );
    let context_messages = manager.context_store.get_context_messages(&session_id);
    assert_eq!(context_messages.len(), 1);
    assert_eq!(
        context_messages[0].metadata.semantic_kind,
        Some(MessageSemanticKind::ActualUserInput)
    );

    let sanitized_snapshot = persistence_manager
        .load_turn_context_snapshot(workspace.path(), &session_id, 1)
        .await
        .expect("snapshot load should succeed")
        .expect("snapshot should still exist");
    assert_eq!(sanitized_snapshot.len(), 1);
    assert_eq!(
        sanitized_snapshot[0].metadata.semantic_kind,
        Some(MessageSemanticKind::ActualUserInput)
    );
}

#[tokio::test]
async fn rollback_sanitizes_pre_cutoff_snapshot_and_truncates_cutoff() {
    use crate::agentic::core::{InternalReminderKind, Message, MessageSemanticKind};

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Rollback sanitize".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    for index in 0..=2 {
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
    }

    {
        let mut active = manager
            .sessions
            .get_mut(&session.session_id)
            .expect("session should be active");
        active.dialog_turn_ids = vec![
            "turn-0".to_string(),
            "turn-1".to_string(),
            "turn-2".to_string(),
        ];
    }

    manager
        .merge_session_custom_metadata(
            &session.session_id,
            json!({
                super::super::LISTING_BASELINE_REBUILD_TURN_INDEX_METADATA_KEY: 2,
            }),
        )
        .await
        .expect("cutoff metadata should save");

    persistence_manager
        .save_turn_context_snapshot(
            workspace.path(),
            &session.session_id,
            0,
            &[
                Message::internal_reminder(
                    InternalReminderKind::AgentListingDiff,
                    "# Agent Listing Update\n\nChanged",
                )
                .with_turn_id("turn-0".to_string()),
                Message::user("prompt 0".to_string())
                    .with_turn_id("turn-0".to_string())
                    .with_semantic_kind(MessageSemanticKind::ActualUserInput),
            ],
        )
        .await
        .expect("snapshot 0 should save");
    persistence_manager
        .save_turn_context_snapshot(
            workspace.path(),
            &session.session_id,
            1,
            &[
                Message::user("prompt 0".to_string()),
                Message::user("prompt 1".to_string()),
            ],
        )
        .await
        .expect("snapshot 1 should save");

    manager
        .rollback_context_to_turn_start(workspace.path(), &session.session_id, 1)
        .await
        .expect("rollback should succeed");

    let context_messages = manager
        .context_store
        .get_context_messages(&session.session_id);
    assert_eq!(context_messages.len(), 1);
    assert_eq!(
        context_messages[0].metadata.semantic_kind,
        Some(MessageSemanticKind::ActualUserInput)
    );

    let sanitized_snapshot = persistence_manager
        .load_turn_context_snapshot(workspace.path(), &session.session_id, 0)
        .await
        .expect("snapshot 0 load should succeed")
        .expect("snapshot 0 should still exist");
    assert_eq!(sanitized_snapshot.len(), 1);
    assert_eq!(
        sanitized_snapshot[0].metadata.semantic_kind,
        Some(MessageSemanticKind::ActualUserInput)
    );

    let metadata = persistence_manager
        .load_session_metadata(workspace.path(), &session.session_id)
        .await
        .expect("metadata load should succeed")
        .expect("metadata should exist");
    assert_eq!(
        SessionManager::listing_baseline_rebuild_turn_index_from_metadata(Some(&metadata)),
        Some(1)
    );
}

#[tokio::test]
async fn skill_agent_baseline_override_snapshot_persists_across_session_restore() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Listing baseline".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should be created");
    let baseline = TurnSkillAgentSnapshot {
        skills: vec![SkillSnapshotEntry {
            name: "skill-a".to_string(),
            description: "desc-a".to_string(),
            location: "/skills/a".to_string(),
        }],
        ..Default::default()
    };

    manager
        .remember_skill_agent_baseline_override_snapshot(&session.session_id, baseline.clone())
        .await;

    let metadata = persistence_manager
        .load_session_metadata(workspace.path(), &session.session_id)
        .await
        .expect("metadata load should succeed")
        .expect("metadata should exist");
    assert_eq!(metadata.custom_metadata, None);
    assert_eq!(
        persistence_manager
            .load_skill_agent_baseline_override_snapshot(workspace.path(), &session.session_id,)
            .await
            .expect("override snapshot load should succeed"),
        Some(baseline.clone())
    );

    manager.evict_loaded_session_for_test(&session.session_id);
    let restored_manager = test_manager(persistence_manager);
    restored_manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("session should restore");

    assert_eq!(
        restored_manager
            .skill_agent_baseline_override_snapshot(&session.session_id)
            .await,
        Some(baseline)
    );
}

#[tokio::test]
async fn seed_forked_skill_agent_listing_baselines_splits_prompt_and_diff_baselines() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = test_manager(persistence_manager.clone());
    let parent = manager
        .create_session(
            "Parent".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("parent session should create");
    let child = manager
        .create_session(
            "Child".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("child session should create");
    let prompt_baseline = TurnSkillAgentSnapshot {
        skills: vec![SkillSnapshotEntry {
            name: "skill-parent-turn-0".to_string(),
            description: "desc-0".to_string(),
            location: "/skills/turn-0".to_string(),
        }],
        ..Default::default()
    };
    let latest_baseline = TurnSkillAgentSnapshot {
        skills: vec![SkillSnapshotEntry {
            name: "skill-parent-latest".to_string(),
            description: "desc-latest".to_string(),
            location: "/skills/latest".to_string(),
        }],
        ..Default::default()
    };

    manager
        .remember_turn_skill_agent_snapshot(&parent.session_id, 0, prompt_baseline.clone())
        .await;
    manager
        .remember_turn_skill_agent_snapshot(&parent.session_id, 2, latest_baseline.clone())
        .await;
    {
        let mut parent_session = manager
            .sessions
            .get_mut(&parent.session_id)
            .expect("parent session should remain in memory");
        parent_session.dialog_turn_ids = vec![
            "turn-0".to_string(),
            "turn-1".to_string(),
            "turn-2".to_string(),
        ];
    }

    manager
        .seed_forked_skill_agent_listing_baselines(&parent.session_id, &child.session_id)
        .await;

    assert_eq!(
        manager
            .skill_agent_baseline_override_snapshot(&child.session_id)
            .await,
        Some(prompt_baseline.clone())
    );
    assert_eq!(
        manager
            .turn_skill_agent_snapshot(&child.session_id, 0)
            .await,
        Some(latest_baseline.clone())
    );

    manager.evict_loaded_session_for_test(&child.session_id);
    let restored_manager = test_manager(persistence_manager);
    restored_manager
        .restore_session(workspace.path(), &child.session_id)
        .await
        .expect("child session should restore");
    assert_eq!(
        restored_manager
            .skill_agent_baseline_override_snapshot(&child.session_id)
            .await,
        Some(prompt_baseline)
    );
    assert_eq!(
        restored_manager
            .turn_skill_agent_snapshot(&child.session_id, 0)
            .await,
        Some(latest_baseline)
    );
}
