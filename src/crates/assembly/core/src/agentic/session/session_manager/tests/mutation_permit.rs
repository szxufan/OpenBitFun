use super::super::SessionManager;
use super::support::{in_memory_test_manager, test_manager, TestWorkspace};
use crate::agentic::core::{CompressionState, SessionConfig};
use crate::agentic::persistence::PersistenceManager;
use dashmap::try_result::TryResult;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn session_creation_waits_for_the_same_session_mutation_permit() {
    let workspace = TestWorkspace::new();
    let manager = Arc::new(in_memory_test_manager());
    let session_id = "serialized-create-session";
    let guard = manager.lock_session_mutation(session_id).await;
    let manager_for_create = manager.clone();
    let workspace_path = workspace.path().to_string_lossy().to_string();

    let create_task = tokio::spawn(async move {
        manager_for_create
            .create_session_with_id(
                Some(session_id.to_string()),
                "Serialized".to_string(),
                "Standard".to_string(),
                SessionConfig {
                    tool_presentation_mode: Default::default(),
                    workspace_path: Some(workspace_path),
                    ..Default::default()
                },
            )
            .await
    });
    tokio::task::yield_now().await;
    assert!(!create_task.is_finished());

    drop(guard);
    create_task
        .await
        .expect("create task should join")
        .expect("create should continue after the permit is released");
}

#[tokio::test]
async fn session_restore_waits_for_the_same_session_mutation_permit() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = Arc::new(test_manager(persistence_manager));
    let session = manager
        .create_session(
            "Persisted".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    manager.evict_loaded_session_for_test(&session.session_id);

    let guard = manager.lock_session_mutation(&session.session_id).await;
    let manager_for_restore = manager.clone();
    let session_id = session.session_id.clone();
    let workspace_path = workspace.path().to_path_buf();
    let restore_task = tokio::spawn(async move {
        manager_for_restore
            .restore_session(&workspace_path, &session_id)
            .await
    });
    tokio::task::yield_now().await;
    assert!(!restore_task.is_finished());

    drop(guard);
    restore_task
        .await
        .expect("restore task should join")
        .expect("restore should continue after the permit is released");
}

#[tokio::test]
async fn session_mode_update_waits_for_the_same_session_mutation_permit() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = Arc::new(test_manager(persistence_manager));
    let session = manager
        .create_session(
            "Serialized mode update".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    let guard = manager.lock_session_mutation(&session.session_id).await;
    let manager_for_update = manager.clone();
    let session_id = session.session_id.clone();
    let update_task = tokio::spawn(async move {
        manager_for_update
            .update_session_agent_type(&session_id, "Cowork")
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!update_task.is_finished());

    drop(guard);
    update_task
        .await
        .expect("update task should join")
        .expect("mode update should continue after the permit is released");
}

#[tokio::test]
async fn compression_update_waits_for_the_same_session_mutation_permit() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = Arc::new(test_manager(persistence_manager));
    let session = manager
        .create_session(
            "Serialized compression update".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    let guard = manager.lock_session_mutation(&session.session_id).await;
    let manager_for_update = manager.clone();
    let session_id = session.session_id.clone();
    let update_task = tokio::spawn(async move {
        manager_for_update
            .update_compression_state(
                &session_id,
                CompressionState {
                    last_compression_at: None,
                    compression_count: 1,
                },
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!update_task.is_finished());

    drop(guard);
    update_task
        .await
        .expect("update task should join")
        .expect("compression update should continue after the permit is released");
}

#[tokio::test]
async fn turn_start_waits_for_the_same_session_mutation_permit() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = Arc::new(test_manager(persistence_manager));
    let session = manager
        .create_session(
            "Serialized turn start".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    let guard = manager.lock_session_mutation(&session.session_id).await;
    let manager_for_turn = manager.clone();
    let session_id = session.session_id.clone();
    let turn_task = tokio::spawn(async move {
        manager_for_turn
            .start_dialog_turn(
                &session_id,
                "Standard".to_string(),
                "hello".to_string(),
                Some("serialized-turn".to_string()),
                None,
                None,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!turn_task.is_finished());

    drop(guard);
    turn_task
        .await
        .expect("turn task should join")
        .expect("turn start should continue after the permit is released");
}

#[tokio::test]
async fn auto_save_interval_waits_before_first_tick() {
    let mut ticker = SessionManager::auto_save_interval(Duration::from_millis(40));
    let started = tokio::time::Instant::now();

    ticker.tick().await;

    assert!(started.elapsed() >= Duration::from_millis(30));
}

#[tokio::test]
async fn auto_save_snapshot_collection_releases_session_map_guards() {
    let workspace = TestWorkspace::new();
    let manager = in_memory_test_manager();
    let session = manager
        .create_session(
            "Auto-save snapshot".to_string(),
            "agent".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    let snapshots = SessionManager::collect_auto_save_snapshots(
        &manager.sessions,
        &manager.transient_session_ids,
    );
    assert!(snapshots
        .iter()
        .any(|snapshot| snapshot.session_id == session.session_id));

    match manager.sessions.try_get_mut(&session.session_id) {
        TryResult::Present(_) => {}
        TryResult::Absent => panic!("session should remain present"),
        TryResult::Locked => panic!("snapshot collection should not retain session map guards"),
    };
}
