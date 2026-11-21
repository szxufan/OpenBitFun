use super::super::test_support::test_persistent_coordinator;
use super::super::SkillAgentSnapshotPersistence;
use super::super::{
    append_skill_agent_listing_diff_reminders, merge_prepended_messages_for_turn,
    plan_skill_agent_listing_update,
};
use crate::agentic::core::{
    InternalReminderKind, Message, MessageContent, SessionConfig, SessionKind,
};
use crate::agentic::session::{SystemPromptCacheIdentity, UserContextCacheIdentity};
use crate::agentic::skill_agent_snapshot::SkillSnapshotEntry;
use crate::agentic::TurnSkillAgentSnapshot;

#[tokio::test]
async fn btw_session_is_ephemeral_and_seeds_forked_listing_baselines() {
    let (coordinator, session_manager) = test_persistent_coordinator();
    // The parent lives in a registered remote workspace; the child must
    // inherit that record's SSH facts rather than transport hints.
    let remote_workspace = crate::service::workspace::legacy_compat::register_remote_fixture(
        &format!("/srv/btw-baseline/{}", uuid::Uuid::new_v4()),
        "ssh-user@example.test:22",
        "example.test",
    )
    .await;

    let parent_session = session_manager
        .create_session(
            "Parent".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_id: Some(remote_workspace.id.clone()),
                workspace_path: Some(remote_workspace.root_path.to_string_lossy().into_owned()),
                remote_connection_id: Some("ssh-user@example.test:22".to_string()),
                remote_ssh_host: Some("example.test".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("parent session should be created");
    session_manager
        .inherit_session_agent_type_state(
            &parent_session.session_id,
            Some("Standard".to_string()),
            Some("Standard".to_string()),
        )
        .await
        .expect("parent agent type state should be set");
    session_manager
        .replace_context_messages(
            &parent_session.session_id,
            vec![crate::agentic::core::Message::user(
                "parent context".to_string(),
            )],
        )
        .await;

    let system_prompt_identity = SystemPromptCacheIdentity::new("template:standard_mode");
    let user_context_identity = UserContextCacheIdentity::new("workspace_context");
    session_manager
        .remember_system_prompt(
            &parent_session.session_id,
            system_prompt_identity.clone(),
            "cached system prompt".to_string(),
        )
        .await;
    session_manager
        .remember_user_context(
            &parent_session.session_id,
            user_context_identity.clone(),
            "cached user context".to_string(),
        )
        .await;

    let baseline_snapshot = TurnSkillAgentSnapshot {
        skills: vec![SkillSnapshotEntry {
            name: "interactive-debug".to_string(),
            description: "debug helper".to_string(),
            location: "C:/Users/wsp/.codex/skills/interactive-debug".to_string(),
        }],
        subagents: Vec::new(),
        deferred_tools: None,
    };
    session_manager
        .remember_turn_skill_agent_snapshot(
            &parent_session.session_id,
            0,
            baseline_snapshot.clone(),
        )
        .await;

    let child_session = coordinator
        .ensure_btw_session(
            &parent_session.session_id,
            "btw-child",
            None,
            "btw-request",
            Some("parent-turn"),
            Some(2),
        )
        .await
        .expect("btw child session should be created");

    assert_eq!(
        child_session.kind,
        crate::agentic::core::SessionKind::EphemeralChild
    );
    assert_eq!(
        child_session.last_user_dialog_agent_type.as_deref(),
        Some("Standard")
    );
    assert_eq!(
        child_session.last_submitted_agent_type.as_deref(),
        Some("Standard")
    );
    assert_eq!(
        child_session.config.remote_connection_id.as_deref(),
        Some("ssh-user@example.test:22")
    );
    assert_eq!(
        child_session.config.remote_ssh_host.as_deref(),
        Some("example.test")
    );
    assert_eq!(
        child_session.config.prompt_cache_lineage_id.as_deref(),
        Some(parent_session.session_id.as_str())
    );
    assert_eq!(
        session_manager
            .cached_system_prompt(&child_session.session_id, &system_prompt_identity)
            .await,
        Some("cached system prompt".to_string())
    );
    assert_eq!(
        session_manager
            .cached_user_context(&child_session.session_id, &user_context_identity)
            .await,
        Some("cached user context".to_string())
    );
    assert_eq!(
        session_manager
            .skill_agent_baseline_override_snapshot(&child_session.session_id)
            .await,
        Some(baseline_snapshot.clone())
    );
    assert_eq!(
        session_manager
            .turn_skill_agent_snapshot(&child_session.session_id, 0)
            .await,
        Some(baseline_snapshot)
    );

    let session_storage_path = session_manager
        .storage_path_binding_for_test(&child_session.session_id)
        .expect("BTW storage path should be bound");
    struct TempStorageGuard(std::path::PathBuf);
    impl Drop for TempStorageGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _storage_guard = TempStorageGuard(session_storage_path.clone());
    let metadata = session_manager
        .load_session_metadata(&session_storage_path, &child_session.session_id)
        .await
        .expect("BTW metadata lookup should succeed");
    assert!(metadata.is_none(), "temporary BTW must not write history");
    assert!(!session_manager.should_persist_session_id(&child_session.session_id));

    let reused = coordinator
        .ensure_btw_session(
            &parent_session.session_id,
            &child_session.session_id,
            None,
            "btw-follow-up",
            Some("parent-turn"),
            Some(2),
        )
        .await
        .expect("temporary BTW should accept follow-up questions");
    assert_eq!(reused.kind, SessionKind::EphemeralChild);
    assert!(session_manager
        .load_session_metadata(&session_storage_path, &child_session.session_id)
        .await
        .expect("BTW metadata lookup after reuse should succeed")
        .is_none());
}

#[test]
fn skill_and_agent_listing_diff_reminders_are_added_when_changed() {
    let mut messages = vec![Message::internal_reminder(
        InternalReminderKind::AgentMode,
        "mode",
    )];

    append_skill_agent_listing_diff_reminders(
        &mut messages,
        Some("skills changed".to_string()),
        Some("agents changed".to_string()),
        Some("deferred tools changed".to_string()),
    );

    assert_eq!(messages.len(), 4);
    assert_eq!(
        messages[0].internal_reminder_kind(),
        Some(InternalReminderKind::AgentMode)
    );
    assert!(messages[1..].iter().all(|message| message
        .internal_reminder_kind()
        .is_some_and(InternalReminderKind::is_listing_diff)));
}

#[test]
fn skill_and_agent_listing_diff_reminders_skip_missing_updates() {
    let mut messages = Vec::new();

    append_skill_agent_listing_diff_reminders(
        &mut messages,
        None,
        Some("agents changed".to_string()),
        None,
    );

    let kinds = messages
        .iter()
        .map(Message::internal_reminder_kind)
        .collect::<Vec<_>>();
    assert_eq!(kinds, vec![Some(InternalReminderKind::AgentListingDiff)]);
}

fn deferred_listing_snapshot(names: &[&str]) -> TurnSkillAgentSnapshot {
    TurnSkillAgentSnapshot {
        deferred_tools: Some(
            names
                .iter()
                .map(
                    |name| crate::agentic::skill_agent_snapshot::DeferredToolSnapshotEntry {
                        name: (*name).to_string(),
                        short_description: None,
                    },
                )
                .collect(),
        ),
        ..Default::default()
    }
}

fn plan_listing_update(
    previous: &TurnSkillAgentSnapshot,
    current: &TurnSkillAgentSnapshot,
    head_lists_deferred_tools: bool,
) -> (SkillAgentSnapshotPersistence, Vec<Message>) {
    let mut messages = Vec::new();
    let persistence = plan_skill_agent_listing_update(
        &mut messages,
        "session",
        3,
        1,
        previous,
        current,
        head_lists_deferred_tools,
    );
    (persistence, messages)
}

#[test]
fn deferred_tool_changes_append_a_listing_diff_when_the_head_lists_the_baseline() {
    let (persistence, messages) = plan_listing_update(
        &deferred_listing_snapshot(&["WebFetch"]),
        &deferred_listing_snapshot(&["WebFetch", "mcp__docs__search"]),
        true,
    );

    assert_eq!(persistence, SkillAgentSnapshotPersistence::SaveCurrentTurn);
    assert_eq!(messages.len(), 1);
    assert!(messages[0]
        .internal_reminder_kind()
        .is_some_and(InternalReminderKind::is_listing_diff));
    let MessageContent::Text(text) = &messages[0].content else {
        panic!("listing diff reminders are text messages");
    };
    assert!(text.contains("# Deferred Tool Listing Update"));
    assert!(text.contains("- mcp__docs__search"));
}

#[test]
fn deferred_tool_changes_are_persisted_without_reminders_when_the_head_renders_current() {
    let (persistence, messages) = plan_listing_update(
        &deferred_listing_snapshot(&["WebFetch"]),
        &deferred_listing_snapshot(&["Cron"]),
        false,
    );

    assert_eq!(persistence, SkillAgentSnapshotPersistence::SaveCurrentTurn);
    assert!(messages.is_empty());
}

#[test]
fn untracked_previous_snapshot_starts_tracking_or_rebuilds_a_tracked_head() {
    let legacy = TurnSkillAgentSnapshot::default();
    let current = deferred_listing_snapshot(&["WebFetch"]);

    let (persistence, messages) = plan_listing_update(&legacy, &current, false);
    assert_eq!(persistence, SkillAgentSnapshotPersistence::SaveCurrentTurn);
    assert!(messages.is_empty());

    let (persistence, messages) = plan_listing_update(&legacy, &current, true);
    assert_eq!(
        persistence,
        SkillAgentSnapshotPersistence::RebuildListingBaseline
    );
    assert!(messages.is_empty());
}

#[test]
fn unchanged_tracked_snapshot_needs_no_persistence_or_reminder() {
    let snapshot = deferred_listing_snapshot(&["WebFetch", "Cron"]);
    let (persistence, messages) = plan_listing_update(&snapshot, &snapshot, true);

    assert_eq!(persistence, SkillAgentSnapshotPersistence::None);
    assert!(messages.is_empty());
}

#[test]
fn merge_prepended_messages_places_scheduled_job_after_mode_reminder() {
    let merged = merge_prepended_messages_for_turn(
        vec![
            Message::internal_reminder(InternalReminderKind::ScheduledJob, "scheduled"),
            Message::internal_reminder(InternalReminderKind::Generic, "generic"),
        ],
        vec![
            Message::internal_reminder(InternalReminderKind::SkillListingDiff, "skills"),
            Message::internal_reminder(InternalReminderKind::AgentMode, "mode"),
        ],
        true,
    );

    let kinds = merged
        .iter()
        .map(|message| message.internal_reminder_kind())
        .collect::<Vec<_>>();

    assert_eq!(
        kinds,
        vec![
            Some(InternalReminderKind::Generic),
            Some(InternalReminderKind::SkillListingDiff),
            Some(InternalReminderKind::AgentMode),
            Some(InternalReminderKind::RemoteFileDelivery),
            Some(InternalReminderKind::ScheduledJob),
        ]
    );
}
