use super::super::{SessionManager, SessionManagerConfig};
use super::support::{
    in_memory_test_manager, test_manager, test_manager_with_config, TestWorkspace,
};
use crate::agentic::core::{ProcessingPhase, Session, SessionConfig, SessionState};
use crate::agentic::persistence::PersistenceManager;
use crate::agentic::session::PromptCachePolicy;
use crate::service::session::SessionKind;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[test]
fn idle_eviction_only_selects_expired_idle_sessions_that_can_be_restored() {
    let now = SystemTime::now();
    let expired_at = now - Duration::from_secs(120);
    let mut durable = Session::new(
        "Durable".to_string(),
        "Standard".to_string(),
        SessionConfig::default(),
    );
    durable.last_activity_at = expired_at;
    let mut transient = Session::new(
        "Connection scoped".to_string(),
        "Standard".to_string(),
        SessionConfig::default(),
    );
    transient.last_activity_at = expired_at;
    let mut processing = Session::new(
        "Processing".to_string(),
        "Standard".to_string(),
        SessionConfig::default(),
    );
    processing.last_activity_at = expired_at;
    processing.state = SessionState::Processing {
        current_turn_id: "active-turn".to_string(),
        phase: ProcessingPhase::Thinking,
    };
    let durable_id = durable.session_id.clone();
    let transient_id = transient.session_id.clone();
    let processing_id = processing.session_id.clone();
    let sessions = DashMap::new();
    sessions.insert(durable_id.clone(), durable);
    sessions.insert(transient_id.clone(), transient);
    sessions.insert(processing_id.clone(), processing);
    let transient_session_ids = DashMap::new();
    transient_session_ids.insert(transient_id.clone(), ());

    let candidates = SessionManager::collect_expired_session_candidates(
        &sessions,
        &transient_session_ids,
        now,
        Duration::from_secs(60),
    );

    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.session_id.as_str())
            .collect::<Vec<_>>(),
        [durable_id.as_str()]
    );
    assert!(sessions.contains_key(&transient_id));
    assert!(sessions.contains_key(&processing_id));

    assert!(SessionManager::collect_expired_session_candidates(
        &sessions,
        &transient_session_ids,
        now,
        Duration::MAX,
    )
    .is_empty());
}

#[test]
fn idle_eviction_rechecks_state_before_removing_candidate() {
    let now = SystemTime::now();
    let expired_at = now - Duration::from_secs(120);
    let mut session = Session::new(
        "Becomes active".to_string(),
        "Standard".to_string(),
        SessionConfig::default(),
    );
    session.last_activity_at = expired_at;
    let session_id = session.session_id.clone();
    let sessions = DashMap::new();
    sessions.insert(session_id.clone(), session);
    let transient_session_ids = DashMap::new();
    let candidate = SessionManager::collect_expired_session_candidates(
        &sessions,
        &transient_session_ids,
        now,
        Duration::from_secs(60),
    )
    .into_iter()
    .next()
    .expect("expired Idle Session should be selected");

    sessions
        .get_mut(&session_id)
        .expect("selected Session")
        .state = SessionState::Processing {
        current_turn_id: "active-turn".to_string(),
        phase: ProcessingPhase::Thinking,
    };

    assert!(SessionManager::cleanup_snapshot_for_candidate(
        &sessions,
        &candidate,
        now,
        Duration::from_secs(60),
    )
    .is_none());
    assert!(sessions
        .remove_if(&session_id, |_, session| {
            SessionManager::cleanup_candidate_matches_session(
                session,
                &candidate,
                now,
                Duration::from_secs(60),
            )
        })
        .is_none());
    assert!(matches!(
        sessions
            .get(&session_id)
            .expect("active Session retained")
            .state,
        SessionState::Processing { .. }
    ));
}

#[tokio::test]
async fn unloading_a_session_releases_capacity_without_deleting_persistence() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager_with_config(
        persistence_manager.clone(),
        SessionManagerConfig {
            max_active_sessions: 1,
            session_idle_timeout: Duration::from_secs(3600),
            auto_save_interval: Duration::from_secs(300),
            enable_persistence: true,
            prompt_cache_policy: PromptCachePolicy::default(),
        },
    );
    let config = SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_path: Some(workspace.path().to_string_lossy().to_string()),
        ..Default::default()
    };
    let first = manager
        .create_session(
            "First loaded session".to_string(),
            "Standard".to_string(),
            config.clone(),
        )
        .await
        .expect("first session should be created");

    assert!(manager
        .unload_session_from_memory(&first.session_id)
        .await
        .expect("session should unload"));
    assert!(manager.get_session(&first.session_id).is_none());
    assert!(
        persistence_manager
            .load_session_metadata(workspace.path(), &first.session_id)
            .await
            .expect("metadata should load")
            .is_some(),
        "unload must preserve persisted history"
    );

    let second = manager
        .create_session(
            "Second loaded session".to_string(),
            "Standard".to_string(),
            config,
        )
        .await
        .expect("unload should release the active-session slot");
    assert_ne!(first.session_id, second.session_id);
}

#[tokio::test]
async fn transient_session_cannot_bypass_owned_discard_through_unload() {
    let workspace = TestWorkspace::new();
    let manager = in_memory_test_manager();
    let session = manager
        .create_transient_session_with_id_and_details(
            None,
            "Connection Session".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
            None,
            SessionKind::Standard,
        )
        .await
        .expect("transient session should create");

    let error = manager
        .unload_session_from_memory(&session.session_id)
        .await
        .expect_err("transient session must use owned discard");
    assert!(error.to_string().contains("transient session"));
    assert!(manager.get_session(&session.session_id).is_some());
    assert!(manager.is_transient_session(&session.session_id));
}

#[tokio::test]
async fn internal_delete_compensation_clears_transient_identity() {
    let workspace = TestWorkspace::new();
    let manager = in_memory_test_manager();
    let session = manager
        .create_transient_session_with_id_and_details(
            None,
            "Prepared Subagent".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
            None,
            SessionKind::Subagent,
        )
        .await
        .expect("transient subagent should create");

    manager
        .delete_session_by_id(&session.session_id)
        .await
        .expect("internal compensation should delete prepared subagent");

    assert!(manager.get_session(&session.session_id).is_none());
    assert!(!manager.is_transient_session(&session.session_id));
}

#[tokio::test]
async fn restores_share_the_same_exact_active_session_capacity_as_creates() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let config = SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_path: Some(workspace.path().to_string_lossy().to_string()),
        ..Default::default()
    };
    let first = Session::new(
        "First persisted".to_string(),
        "Standard".to_string(),
        config.clone(),
    );
    let second = Session::new(
        "Second persisted".to_string(),
        "Standard".to_string(),
        config,
    );
    persistence_manager
        .save_session(workspace.path(), &first)
        .await
        .expect("first fixture should persist");
    persistence_manager
        .save_session(workspace.path(), &second)
        .await
        .expect("second fixture should persist");
    let manager = test_manager_with_config(
        persistence_manager,
        SessionManagerConfig {
            max_active_sessions: 1,
            enable_persistence: true,
            ..Default::default()
        },
    );

    manager
        .restore_session(workspace.path(), &first.session_id)
        .await
        .expect("first restore should reserve the only slot");
    let error = manager
        .restore_session(workspace.path(), &second.session_id)
        .await
        .expect_err("second restore must respect active-session capacity");
    assert!(error.to_string().contains("maximum session limit"));

    manager
        .unload_session_from_memory(&first.session_id)
        .await
        .expect("first session should unload");
    manager
        .restore_session(workspace.path(), &second.session_id)
        .await
        .expect("unload should release capacity for a later restore");
}

#[tokio::test]
async fn concurrent_creates_cannot_overbook_active_session_capacity() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = Arc::new(test_manager_with_config(
        persistence_manager,
        SessionManagerConfig {
            max_active_sessions: 1,
            enable_persistence: true,
            ..Default::default()
        },
    ));
    let config = SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_path: Some(workspace.path().to_string_lossy().to_string()),
        ..Default::default()
    };

    let first = {
        let manager = manager.clone();
        let config = config.clone();
        tokio::spawn(async move {
            manager
                .create_session_with_id(
                    Some("capacity-first".to_string()),
                    "First".to_string(),
                    "Standard".to_string(),
                    config,
                )
                .await
        })
    };
    let second = {
        let manager = manager.clone();
        tokio::spawn(async move {
            manager
                .create_session_with_id(
                    Some("capacity-second".to_string()),
                    "Second".to_string(),
                    "Standard".to_string(),
                    config,
                )
                .await
        })
    };
    let first = first.await.expect("first create task should join");
    let second = second.await.expect("second create task should join");

    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(manager.sessions.len(), 1);
    assert_eq!(manager.active_session_permits.len(), 1);
}

#[tokio::test]
async fn failed_unavailable_mode_migration_does_not_publish_the_restored_session() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let session_id = uuid::Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        session_id.clone(),
        "Unavailable mode".to_string(),
        "removed-mode-that-cannot-exist".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().to_string()),
            ..Default::default()
        },
    );
    persistence_manager
        .save_session(workspace.path(), &session)
        .await
        .expect("invalid historical mode fixture should persist");
    persistence_manager.fail_next_session_metadata_write_for_test(&session_id);
    let manager = test_manager_with_config(
        persistence_manager,
        SessionManagerConfig {
            enable_persistence: true,
            ..Default::default()
        },
    );

    let error = manager
        .restore_session(workspace.path(), &session_id)
        .await
        .expect_err("mode migration write failure must fail restore");

    assert!(error.to_string().contains("Injected session metadata"));
    assert!(
        manager.get_session(&session_id).is_none(),
        "failed migration must not consume active-session capacity"
    );
    assert!(manager.active_session_permits.is_empty());
    assert!(manager
        .session_storage_path_index
        .get(&session_id)
        .is_none());
}

#[tokio::test]
async fn failed_restore_state_write_does_not_publish_context_or_capacity() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let session_id = uuid::Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        session_id.clone(),
        "Unavailable mode".to_string(),
        "removed-mode-that-cannot-exist".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().to_string()),
            ..Default::default()
        },
    );
    persistence_manager
        .save_session(workspace.path(), &session)
        .await
        .expect("historical session fixture should persist");
    persistence_manager.fail_next_session_state_write_for_test(&session_id);
    let manager = test_manager(persistence_manager);

    let error = manager
        .restore_session(workspace.path(), &session_id)
        .await
        .expect_err("state migration write failure must fail restore");

    assert!(error.to_string().contains("Injected session state"));
    assert!(manager.get_session(&session_id).is_none());
    assert!(manager.active_session_permits.is_empty());
    assert!(manager
        .session_storage_path_index
        .get(&session_id)
        .is_none());
    assert!(!manager.context_store.has_session(&session_id));
}
