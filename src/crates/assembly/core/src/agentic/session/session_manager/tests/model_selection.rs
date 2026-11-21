use super::support::{test_manager, TestWorkspace};
use crate::agentic::core::{ProcessingPhase, Session, SessionConfig, SessionState};
use crate::agentic::persistence::PersistenceManager;
use crate::service::session::SessionKind;
use crate::util::errors::OpenBitFunError;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn cold_session_model_read_ignores_history_and_context_without_activating_runtime() {
    use openbitfun_core_types::model_pool::{ModelPoolBinding, ModelSelection};
    use openbitfun_services_core::session::SessionStorageLayout;

    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let persistence = Arc::new(PersistenceManager::new(paths.clone()).unwrap());
    let manager = test_manager(persistence.clone());
    let id = Uuid::new_v4().to_string();
    let binding = ModelPoolBinding {
        pool_id: "pool".into(),
        model_id: "chosen-member".into(),
        configuration_fingerprint: "current-binding".into(),
        generation: 7,
    };
    let session = Session::new_with_id(
        id.clone(),
        "Cold selector read".into(),
        "Standard".into(),
        SessionConfig {
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
            model_id: Some("primary".into()),
            model_pool_id: Some("pool".into()),
            model_pool_binding: Some(binding.clone()),
            reasoning_preset: Some("high".into()),
            model_routing: Some("manual".into()),
            ..Default::default()
        },
    );
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    let storage_path = paths.project_sessions_dir(workspace.path());
    let layout = SessionStorageLayout::new(&storage_path);
    std::fs::create_dir_all(layout.turns_dir(&id)).unwrap();
    std::fs::create_dir_all(layout.snapshots_dir(&id)).unwrap();
    let unreadable = "unreadable unrelated historical content".repeat(32_768);
    std::fs::write(layout.turn_path(&id, 0), &unreadable).unwrap();
    std::fs::write(layout.context_snapshot_path(&id, 0), &unreadable).unwrap();
    let state_bytes = std::fs::read(layout.state_path(&id)).unwrap();
    let metadata_bytes = std::fs::read(layout.metadata_path(&id)).unwrap();

    let header = manager
        .read_session_header(Some(&storage_path), &id, false)
        .await
        .unwrap();
    assert!(header.dialog_turn_ids.is_empty());
    assert_eq!(header.config.model_routing.as_deref(), Some("manual"));
    let target = super::super::session_model_target(header);
    assert_eq!(
        target.selection,
        ModelSelection::Pool {
            pool_id: "pool".into()
        }
    );
    assert_eq!(target.binding, Some(binding));
    assert_eq!(target.reasoning_preset.as_deref(), Some("high"));
    assert!(manager.get_session(&id).is_none());
    assert!(!manager
        .is_session_loaded_from_storage_path(&storage_path, &id)
        .unwrap());
    assert!(manager.active_session_permits.is_empty());
    assert!(manager.session_write_locks.is_empty());
    assert!(!manager.context_store.has_session(&id));
    assert!(!manager.session_storage_path_index.contains_key(&id));
    assert_eq!(std::fs::read(layout.state_path(&id)).unwrap(), state_bytes);
    assert_eq!(
        std::fs::read(layout.metadata_path(&id)).unwrap(),
        metadata_bytes
    );
    assert_eq!(
        std::fs::read_to_string(layout.turn_path(&id, 0)).unwrap(),
        unreadable
    );
    assert_eq!(
        std::fs::read_to_string(layout.context_snapshot_path(&id, 0)).unwrap(),
        unreadable
    );
}

#[tokio::test]
async fn session_model_read_prefers_live_selection_without_waiting_for_turn_or_ignoring_scope() {
    use openbitfun_core_types::model_pool::ModelSelection;

    let workspace = TestWorkspace::new();
    let persistence = Arc::new(PersistenceManager::new(workspace.path_manager()).unwrap());
    let manager = test_manager(persistence);
    let id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        id.clone(),
        "Live selector".into(),
        "Standard".into(),
        SessionConfig {
            model_id: Some("live-model".into()),
            reasoning_preset: Some("low".into()),
            ..Default::default()
        },
    );
    session.state = SessionState::Processing {
        current_turn_id: "running".into(),
        phase: ProcessingPhase::Thinking,
    };
    manager.sessions.insert(id.clone(), session);
    manager.bind_session_storage_path_committed(&id, workspace.path().to_path_buf());
    // No persisted session exists. A running Turn holds this permit until
    // completion; reading the selector must still return its live facts.
    let _turn = manager.acquire_session_mutation(&id).await.unwrap();
    let header = tokio::time::timeout(
        Duration::from_secs(1),
        manager.read_session_header(Some(workspace.path()), &id, false),
    )
    .await
    .expect("a model read must not wait for the running Turn")
    .unwrap();
    assert!(matches!(header.state, SessionState::Processing { .. }));
    let target = super::super::session_model_target(header);
    assert_eq!(
        target.selection,
        ModelSelection::Model {
            model_ref: "live-model".into()
        }
    );
    assert_eq!(target.reasoning_preset.as_deref(), Some("low"));
    assert!(manager.read_session_header(None, &id, false).await.is_ok());
    assert!(manager
        .read_session_header(Some(&workspace.path().join("other")), &id, false)
        .await
        .is_err());
    manager.sessions.get_mut(&id).unwrap().kind = SessionKind::Subagent;
    assert!(manager.read_session_header(None, &id, false).await.is_err());
    assert!(manager.read_session_header(None, &id, true).await.is_ok());
}

#[tokio::test]
async fn cold_session_model_read_preserves_legacy_metadata_and_internal_visibility() {
    use openbitfun_core_types::model_pool::{ModelSelection, SessionModelTarget};
    use openbitfun_services_core::session::SessionStorageLayout;

    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let persistence = Arc::new(PersistenceManager::new(paths.clone()).unwrap());
    let manager = test_manager(persistence.clone());
    let id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        id.clone(),
        "Legacy selector".into(),
        "Standard".into(),
        SessionConfig {
            workspace_path: Some(workspace.path().to_string_lossy().into()),
            model_id: Some("fast".into()),
            ..Default::default()
        },
    );
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    let storage_path = paths.project_sessions_dir(workspace.path());
    let layout = SessionStorageLayout::new(&storage_path);
    std::fs::remove_file(layout.state_path(&id)).unwrap();
    let metadata_bytes = std::fs::read(layout.metadata_path(&id)).unwrap();
    let target = super::super::session_model_target(
        manager
            .read_session_header(Some(&storage_path), &id, false)
            .await
            .unwrap(),
    );
    assert_eq!(
        target.selection,
        ModelSelection::Model {
            model_ref: "fast".into()
        }
    );
    assert_eq!(target.reasoning_preset, None);
    assert_eq!(target.binding, None);
    let old_payload =
        json!({ "sessionId": id, "selection": { "kind": "model", "model_ref": "fast" } });
    let old_target: SessionModelTarget = serde_json::from_value(old_payload).unwrap();
    assert_eq!(target, old_target);
    assert_eq!(
        serde_json::from_value::<SessionModelTarget>(serde_json::to_value(&target).unwrap())
            .unwrap(),
        old_target
    );
    assert!(!layout.state_path(&id).exists());
    assert_eq!(
        std::fs::read(layout.metadata_path(&id)).unwrap(),
        metadata_bytes
    );
    assert!(manager.read_session_header(None, &id, false).await.is_err());
    assert!(manager
        .read_session_header(Some(&storage_path), "missing-session", false)
        .await
        .is_err());
    session.kind = SessionKind::Subagent;
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    assert!(manager
        .read_session_header(Some(&storage_path), &id, false)
        .await
        .is_err());
    assert!(manager
        .read_session_header(Some(&storage_path), &id, true)
        .await
        .is_ok());
    // Pre-kind releases leaked internal children as Standard sessions.
    // Keep the metadata owner's compatibility rule for both cold and live reads.
    session.kind = SessionKind::Standard;
    session.created_by = Some("session-parent".into());
    session.session_name = "Subagent: legacy child".into();
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    assert!(manager
        .read_session_header(Some(&storage_path), &id, false)
        .await
        .is_err());
    assert!(manager
        .read_session_header(Some(&storage_path), &id, true)
        .await
        .is_ok());
    manager.sessions.insert(id.clone(), session);
    assert!(manager.read_session_header(None, &id, false).await.is_err());
    assert!(manager.read_session_header(None, &id, true).await.is_ok());
    manager.sessions.remove(&id);
    assert!(manager.get_session(&id).is_none());
}

#[tokio::test]
async fn cold_session_model_update_preserves_history_and_unknown_state_without_restore() {
    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let persistence = Arc::new(PersistenceManager::new(paths.clone()).unwrap());
    let manager = test_manager(persistence.clone());
    let id = Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        id.clone(),
        "Cold selector".into(),
        "Standard".into(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
            model_id: Some("primary".into()),
            ..Default::default()
        },
    );
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    let directory = paths.project_sessions_dir(workspace.path()).join(&id);
    let state_path = directory.join("state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    state["future_state"] = serde_json::json!({"preserve": true});
    state["config"]["future_config"] = serde_json::json!(42);
    std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    // A corrupt turn body would make a full restore fail. Selector writes
    // must leave transcript bytes alone, including unknown legacy content.
    std::fs::create_dir_all(directory.join("turns")).unwrap();
    let turn_path = directory.join("turns").join("untouched.json");
    std::fs::write(&turn_path, b"unreadable historical turn").unwrap();
    let update = manager
        .update_session_model_selection_in_storage(
            Some(workspace.path()),
            &id,
            "fast",
            Some("high"),
            false,
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(update.config.reasoning_preset.as_deref(), Some("high"));
    assert!(manager.get_session(&id).is_none());
    assert!(manager.active_session_permits.is_empty());
    assert!(!manager.context_store.has_session(&id));
    let updated: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(updated["future_state"], state["future_state"]);
    assert_eq!(updated["config"]["future_config"], 42);
    assert_eq!(updated["config"]["model_id"], "fast");
    assert_eq!(
        persistence
            .load_session_metadata(workspace.path(), &id)
            .await
            .unwrap()
            .unwrap()
            .model_name,
        "fast"
    );
    assert_eq!(
        std::fs::read(&turn_path).unwrap(),
        b"unreadable historical turn"
    );
    let bytes = std::fs::read(&state_path).unwrap();
    manager
        .update_session_model_selection_in_storage(
            Some(workspace.path()),
            &id,
            "fast",
            Some("high"),
            false,
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(&state_path).unwrap(), bytes);
    manager
        .update_session_model_selection_in_storage_with_routing(
            Some(workspace.path()),
            &id,
            "fast",
            Some("high"),
            Some("manual"),
            false,
            |_| Ok(()),
        )
        .await
        .unwrap();
    let routed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(routed["config"]["model_routing"], "manual");
    assert_eq!(routed["future_state"], state["future_state"]);
    assert_eq!(routed["config"]["future_config"], 42);
    assert!(manager
        .update_session_model_selection_in_storage_with_routing(
            Some(workspace.path()),
            &id,
            "fast",
            Some("high"),
            Some("future-mode"),
            false,
            |_| Ok(())
        )
        .await
        .is_err());
    // Clearing a preset on the same model is a real mutation.
    manager
        .update_session_model_selection_in_storage(
            Some(workspace.path()),
            &id,
            "fast",
            None,
            false,
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert!(persistence
        .load_session_header(workspace.path(), &id)
        .await
        .unwrap()
        .config
        .reasoning_preset
        .is_none());
    persistence.fail_next_session_state_write_for_test(&id);
    manager
        .update_session_model_selection_in_storage(
            Some(workspace.path()),
            &id,
            "primary",
            None,
            false,
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        persistence
            .load_session_header(workspace.path(), &id)
            .await
            .unwrap()
            .config
            .model_id
            .as_deref(),
        Some("fast")
    );
    let state_before_failure = std::fs::read(&state_path).unwrap();
    persistence.fail_next_session_metadata_write_for_test(&id);
    assert!(manager
        .update_session_model_selection_in_storage(
            Some(workspace.path()),
            &id,
            "primary",
            None,
            false,
            |_| Ok(())
        )
        .await
        .is_err());
    assert_eq!(std::fs::read(&state_path).unwrap(), state_before_failure);
    assert_eq!(
        persistence
            .load_session_metadata(workspace.path(), &id)
            .await
            .unwrap()
            .unwrap()
            .model_name,
        "fast"
    );
    assert!(manager
        .update_session_model_selection_in_storage(
            Some(workspace.path()),
            &id,
            "primary",
            None,
            false,
            |_| Err(OpenBitFunError::Validation("binding denied".into()))
        )
        .await
        .is_err());
    let writer = persistence
        .lock_session_writes(workspace.path(), &id)
        .unwrap();
    assert!(manager
        .update_session_model_selection_in_storage(
            Some(workspace.path()),
            &id,
            "primary",
            None,
            false,
            |_| Ok(())
        )
        .await
        .is_err());
    drop(writer);
    assert_eq!(std::fs::read(&state_path).unwrap(), state_before_failure);
    // A restart may encounter a metadata projection from before a commit.
    persistence
        .update_session_metadata(workspace.path(), &id, |metadata| {
            metadata.model_name = "primary".into()
        })
        .await
        .unwrap();
    manager
        .update_session_model_selection_in_storage(
            Some(workspace.path()),
            &id,
            "fast",
            None,
            false,
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(
        persistence
            .load_session_metadata(workspace.path(), &id)
            .await
            .unwrap()
            .unwrap()
            .model_name,
        "fast"
    );
}

#[tokio::test]
async fn cold_session_model_update_upgrades_metadata_only_and_rejects_missing_or_internal_sessions()
{
    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let persistence = Arc::new(PersistenceManager::new(paths.clone()).unwrap());
    let manager = test_manager(persistence.clone());
    let id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        id.clone(),
        "Legacy".into(),
        "Standard".into(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
            model_id: Some("primary".into()),
            ..Default::default()
        },
    );
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    std::fs::remove_file(
        paths
            .project_sessions_dir(workspace.path())
            .join(&id)
            .join("state.json"),
    )
    .unwrap();
    manager
        .update_session_model_selection_in_storage(
            Some(workspace.path()),
            &id,
            "fast",
            None,
            false,
            |_| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(
        persistence
            .load_session(workspace.path(), &id)
            .await
            .unwrap()
            .config
            .model_id
            .as_deref(),
        Some("fast")
    );
    session.kind = SessionKind::Subagent;
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    assert!(manager
        .update_session_model_selection_in_storage(
            Some(workspace.path()),
            &id,
            "fast",
            None,
            false,
            |_| Ok(())
        )
        .await
        .is_err());
    assert!(manager
        .update_session_model_selection_in_storage(
            Some(workspace.path()),
            "missing-session",
            "fast",
            None,
            false,
            |_| Ok(())
        )
        .await
        .is_err());
    assert!(!paths
        .project_sessions_dir(workspace.path())
        .join("missing-session")
        .join("metadata.json")
        .exists());
}

#[tokio::test]
async fn session_model_update_is_restored_from_persistence() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Persisted model update".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                model_id: Some("primary".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    manager
        .update_session_model_id(&session.session_id, "fast")
        .await
        .expect("model update should persist");
    manager.evict_loaded_session_for_test(&session.session_id);

    let restored = manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("session should restore from persistence");
    assert_eq!(restored.config.model_id.as_deref(), Some("fast"));
}

#[tokio::test]
async fn failed_session_model_update_preserves_runtime_and_persisted_selector() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Failed model update".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                model_id: Some("primary".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    persistence_manager.fail_next_session_state_write_for_test(&session.session_id);

    manager
        .update_session_model_id(&session.session_id, "fast")
        .await
        .expect_err("failed persistence must reject the model update");
    assert_eq!(
        manager
            .get_session(&session.session_id)
            .expect("session remains loaded")
            .config
            .model_id
            .as_deref(),
        Some("primary")
    );

    manager.evict_loaded_session_for_test(&session.session_id);
    let restored = manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("failed update should restore the previous model");
    assert_eq!(restored.config.model_id.as_deref(), Some("primary"));
}
