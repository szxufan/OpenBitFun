use super::super::SessionManager;
use super::support::{test_manager, TestWorkspace};
use crate::agentic::core::{Message, MessageContent, Session, SessionConfig, ToolCall};
use crate::agentic::persistence::PersistenceManager;
use crate::service::session::{
    DialogTurnData, ModelRoundData, ToolCallData, ToolItemData, ToolResultData, UserMessageData,
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn restore_session_view_loads_turns_without_restoring_runtime_context() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session_id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        session_id.clone(),
        "Large history".to_string(),
        "agent".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().to_string()),
            ..Default::default()
        },
    );
    session.dialog_turn_ids = vec!["turn-1".to_string()];

    persistence_manager
        .save_session(workspace.path(), &session)
        .await
        .expect("session should save");
    let turn = DialogTurnData::new(
        "turn-1".to_string(),
        0,
        session_id.clone(),
        UserMessageData {
            id: "turn-1-user".to_string(),
            content: "hello".to_string(),
            timestamp: 1,
            metadata: None,
        },
    );
    persistence_manager
        .save_dialog_turn(workspace.path(), &turn)
        .await
        .expect("turn should save");
    persistence_manager
        .save_turn_context_snapshot(
            workspace.path(),
            &session_id,
            0,
            &[Message::user("snapshot prompt".to_string())],
        )
        .await
        .expect("context snapshot should save");

    let (view_session, turns) = manager
        .restore_session_view(workspace.path(), &session_id)
        .await
        .expect("session view should restore");

    assert_eq!(view_session.dialog_turn_ids, vec!["turn-1".to_string()]);
    assert_eq!(turns.len(), 1);
    assert!(manager.get_session(&session_id).is_none());
    assert!(manager
        .context_store
        .get_context_messages(&session_id)
        .is_empty());
}

#[tokio::test]
async fn restore_session_repairs_and_persists_incomplete_tool_calls() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session_id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        session_id.clone(),
        "Interrupted tool call".to_string(),
        "Standard".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().to_string()),
            ..Default::default()
        },
    );
    session.dialog_turn_ids = vec!["turn-1".to_string()];
    persistence_manager
        .save_session(workspace.path(), &session)
        .await
        .expect("session should save");

    let mut turn = DialogTurnData::new(
        "turn-1".to_string(),
        0,
        session_id.clone(),
        UserMessageData {
            id: "turn-1-user".to_string(),
            content: "read the file".to_string(),
            timestamp: 1,
            metadata: None,
        },
    );
    turn.mark_completed();
    persistence_manager
        .save_dialog_turn(workspace.path(), &turn)
        .await
        .expect("turn should save");

    let assistant = Message::assistant_with_tools(
        String::new(),
        vec![ToolCall {
            tool_id: "call-1".to_string(),
            tool_name: "Read".to_string(),
            arguments: json!({"path": "a.txt"}),
            ..Default::default()
        }],
    )
    .with_turn_id("turn-1".to_string())
    .with_round_id("round-1".to_string());
    persistence_manager
        .save_turn_context_snapshot(workspace.path(), &session_id, 0, &[assistant])
        .await
        .expect("incomplete snapshot should save");

    manager
        .restore_session(workspace.path(), &session_id)
        .await
        .expect("session should restore");

    let restored = manager
        .get_context_messages(&session_id)
        .await
        .expect("context should load");
    assert_eq!(restored.len(), 2);
    assert!(matches!(
        &restored[1].content,
        MessageContent::ToolResult { tool_id, result, .. }
            if tool_id == "call-1"
                && result == &json!("Tool execution was still in progress when the application closed; the result is unknown. Verify any external side effects before continuing.")
    ));

    let persisted = persistence_manager
        .load_turn_context_snapshot(workspace.path(), &session_id, 0)
        .await
        .expect("repaired snapshot should load")
        .expect("repaired snapshot should exist");
    assert_eq!(persisted.len(), 2);
    assert!(matches!(
        &persisted[1].content,
        MessageContent::ToolResult { tool_id, .. } if tool_id == "call-1"
    ));
}

#[tokio::test]
async fn restore_session_view_tail_with_staged_revert_reads_only_visible_tail() {
    use crate::agentic::session::revert::{
        SessionRevertPhase, SessionRevertState, SESSION_REVERT_SCHEMA_VERSION,
    };
    use openbitfun_services_core::session::SessionStorageLayout;

    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let persistence = Arc::new(PersistenceManager::new(paths.clone()).unwrap());
    let manager = test_manager(persistence.clone());
    let session_id = Uuid::new_v4().to_string();
    let session = Session::new_with_id(
        session_id.clone(),
        "Visible history tail".into(),
        "Standard".into(),
        SessionConfig {
            workspace_path: Some(workspace.path().to_string_lossy().into()),
            ..Default::default()
        },
    );
    persistence
        .save_session(workspace.path(), &session)
        .await
        .unwrap();
    // A sparse legacy sequence proves the boundary is a storage index,
    // while total_turn_count and tail selection use visible ordinals.
    for index in [0, 2, 4, 7] {
        let mut turn = DialogTurnData::new(
            format!("turn-{index}"),
            index,
            session_id.clone(),
            UserMessageData {
                id: format!("user-{index}"),
                content: format!("prompt {index}"),
                timestamp: index as u64,
                metadata: None,
            },
        );
        turn.mark_completed();
        persistence
            .save_dialog_turn(workspace.path(), &turn)
            .await
            .unwrap();
    }
    let storage_path = paths.project_sessions_dir(workspace.path());
    let layout = SessionStorageLayout::new(&storage_path);
    let answer = "complete last visible answer\n".repeat(8192);
    let rounds = SessionManager::build_model_rounds_from_messages(
        &[Message::assistant(answer.clone()).with_round_id("visible-round".into())],
        "turn-2",
        2,
    );
    // Keep an old persisted shape: no kind, recovery, usage or finish fields.
    let legacy_turn = json!({
        "schema_version": 1,
        "turnId": "turn-2", "turnIndex": 2, "sessionId": session_id,
        "timestamp": 2, "startTime": 2, "status": "completed",
        "userMessage": { "id": "user-2", "content": "visible prompt", "timestamp": 2 },
        "modelRounds": rounds,
    });
    std::fs::write(
        layout.turn_path(&session_id, 2),
        serde_json::to_vec(&legacy_turn).unwrap(),
    )
    .unwrap();
    std::fs::remove_file(layout.turn_catalog_path(&session_id)).unwrap();
    // Neither unrelated older content nor the hidden suffix may be parsed.
    for index in [0, 4, 7] {
        std::fs::write(
            layout.turn_path(&session_id, index),
            "unreadable unrelated turn",
        )
        .unwrap();
    }
    let mut revert = SessionRevertState {
        schema_version: SESSION_REVERT_SCHEMA_VERSION,
        boundary_turn: 4,
        original_turn_end: 8,
        phase: SessionRevertPhase::Staged,
        workspace_checkpoint: Vec::new(),
    };
    persistence
        .save_session_revert_state(workspace.path(), &session_id, &revert)
        .await
        .unwrap();

    // Local path resolution and the resolved host-storage entry used by
    // Peer callers must share the same bounded restore behavior.
    for resolved_storage in [false, true] {
        let (view, turns, total, timing) = if resolved_storage {
            manager
                .restore_session_view_from_storage_path_tail_timed(&storage_path, &session_id, 1)
                .await
        } else {
            manager
                .restore_session_view_tail_timed(workspace.path(), &session_id, 1)
                .await
        }
        .expect("only the requested visible Turn should be read");
        assert_eq!(total, 2);
        assert_eq!(timing.turn_load.requested_tail_turn_count, Some(1));
        assert_eq!(timing.turn_load.loaded_turn_count, 1);
        assert_eq!(view.dialog_turn_ids, ["turn-2"]);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].turn_index, 2);
        assert_eq!(turns[0].model_rounds[0].text_items[0].content, answer);
        let round_trip: DialogTurnData =
            serde_json::from_value(serde_json::to_value(&turns[0]).unwrap()).unwrap();
        assert_eq!(round_trip.model_rounds[0].text_items[0].content, answer);
        let catalog = persistence
            .load_session_turn_catalog(&storage_path, &session_id, &turns, total)
            .await
            .unwrap();
        assert_eq!(
            catalog
                .entries
                .iter()
                .map(|entry| entry.storage_turn_index)
                .collect::<Vec<_>>(),
            [0, 2]
        );
        assert!(manager.get_session(&session_id).is_none());
    }

    let (_, turns, total, _) = manager
        .restore_session_view_tail_timed(workspace.path(), &session_id, 0)
        .await
        .unwrap();
    assert!(turns.is_empty());
    assert_eq!(total, 2);
    revert.boundary_turn = 0;
    persistence
        .save_session_revert_state(workspace.path(), &session_id, &revert)
        .await
        .unwrap();
    let (_, turns, total, _) = manager
        .restore_session_view_tail_timed(workspace.path(), &session_id, 1)
        .await
        .unwrap();
    assert!(turns.is_empty());
    assert_eq!(total, 0);
    for index in [0, 4, 7] {
        assert_eq!(
            std::fs::read_to_string(layout.turn_path(&session_id, index)).unwrap(),
            "unreadable unrelated turn"
        );
    }
}

#[tokio::test]
async fn legacy_auto_restore_session_view_uses_primary_without_writing_history() {
    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let persistence = Arc::new(PersistenceManager::new(paths.clone()).unwrap());
    let manager = test_manager(persistence.clone());
    for (metadata_model, state_model, agent, expected) in [
        ("auto", Some("auto"), "Standard", "primary"),
        ("auto", None, "Standard", "primary"),
        ("auto", Some("fast"), "Standard", "fast"),
        ("fast", Some("auto"), "Standard", "primary"),
        ("auto", Some("auto"), "acp:codex", "auto"),
        (
            "removed-model",
            Some("removed-model"),
            "Standard",
            "removed-model",
        ),
    ] {
        let id = Uuid::new_v4().to_string();
        let session = Session::new_with_id(
            id.clone(),
            "Legacy model".into(),
            agent.into(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                model_id: Some(metadata_model.into()),
                ..Default::default()
            },
        );
        persistence
            .save_session(workspace.path(), &session)
            .await
            .unwrap();
        let directory = paths.project_sessions_dir(workspace.path()).join(&id);
        let metadata_path = directory.join("metadata.json");
        let state_path = directory.join("state.json");
        if let Some(model) = state_model {
            let mut state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
            state["config"]["model_id"] = serde_json::json!(model);
            state["config"]
                .as_object_mut()
                .unwrap()
                .remove("model_binding_policy");
            std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
        } else {
            std::fs::remove_file(&state_path).unwrap();
        }
        let metadata_bytes = std::fs::read(&metadata_path).unwrap();
        let state_bytes = std::fs::read(&state_path).ok();
        for restored in [
            manager
                .restore_session_view(workspace.path(), &id)
                .await
                .unwrap()
                .0,
            manager
                .restore_session_view_tail(workspace.path(), &id, 1)
                .await
                .unwrap()
                .0,
        ] {
            assert_eq!(restored.config.model_id.as_deref(), Some(expected));
        }
        assert!(manager.get_session(&id).is_none());
        assert_eq!(std::fs::read(&metadata_path).unwrap(), metadata_bytes);
        assert_eq!(std::fs::read(&state_path).ok(), state_bytes);
        // A normal save persists the compatible selector for subsequent readers.
        let restored = persistence
            .load_session(workspace.path(), &id)
            .await
            .unwrap();
        persistence
            .save_session(workspace.path(), &restored)
            .await
            .unwrap();
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
        assert_eq!(state["config"]["model_id"], expected);
    }
}

#[tokio::test]
async fn restore_session_view_preserves_full_visible_tool_result_payload() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session_id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        session_id.clone(),
        "History with tool output".to_string(),
        "agent".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().to_string()),
            ..Default::default()
        },
    );
    session.dialog_turn_ids = vec!["turn-1".to_string()];

    persistence_manager
        .save_session(workspace.path(), &session)
        .await
        .expect("session should save");

    let visible_output = "complete visible output ".repeat(128);
    let assistant_output = "assistant visible summary ".repeat(16);
    let mut turn = DialogTurnData::new(
        "turn-1".to_string(),
        0,
        session_id.clone(),
        UserMessageData {
            id: "turn-1-user".to_string(),
            content: "show full output".to_string(),
            timestamp: 1,
            metadata: None,
        },
    );
    turn.model_rounds.push(ModelRoundData {
        id: "round-1".to_string(),
        turn_id: "turn-1".to_string(),
        round_index: 0,
        round_group_id: None,
        timestamp: 1,
        text_items: vec![],
        tool_items: vec![ToolItemData {
            id: "tool-1".to_string(),
            tool_name: "ExecCommand".to_string(),
            tool_call: ToolCallData {
                id: "call-1".to_string(),
                input: json!({ "cmd": "printf output" }),
            },
            tool_result: Some(ToolResultData {
                result: json!({
                    "stdout": visible_output,
                    "nested": {
                        "stderr": "also visible",
                    },
                }),
                success: true,
                result_for_assistant: Some(assistant_output.clone()),
                image_attachments: None,
                error: None,
                duration_ms: Some(1),
            }),
            ai_intent: None,
            start_time: 1,
            end_time: Some(2),
            duration_ms: Some(1),
            queue_wait_ms: None,
            preflight_ms: None,
            confirmation_wait_ms: None,
            execution_ms: None,
            order_index: None,
            is_subagent_item: None,
            parent_task_tool_id: None,
            subagent_session_id: None,
            subagent_dialog_turn_id: None,
            attempt_id: None,
            attempt_index: None,
            subagent_model_id: None,
            subagent_model_display_name: None,
            status: Some("completed".to_string()),
            interruption_reason: None,
        }],
        thinking_items: vec![],
        start_time: 1,
        end_time: Some(2),
        duration_ms: Some(1),
        provider_id: None,
        model_config_id: None,
        effective_model_name: None,
        routing: None,
        first_chunk_ms: None,
        first_visible_output_ms: None,
        stream_duration_ms: None,
        ttft_ms: None,
        attempt_count: None,
        attempt_diagnostics: vec![],
        failure_category: None,
        token_details: None,
        status: "completed".to_string(),
    });
    persistence_manager
        .save_dialog_turn(workspace.path(), &turn)
        .await
        .expect("turn should save");

    let (view_session, turns) = manager
        .restore_session_view(workspace.path(), &session_id)
        .await
        .expect("session view should restore");

    let restored_result = turns[0].model_rounds[0].tool_items[0]
        .tool_result
        .as_ref()
        .expect("tool result should be preserved");
    assert_eq!(view_session.dialog_turn_ids, vec!["turn-1".to_string()]);
    assert_eq!(
        restored_result.result["stdout"].as_str(),
        Some(visible_output.as_str())
    );
    assert_eq!(
        restored_result.result["nested"]["stderr"].as_str(),
        Some("also visible")
    );
    assert_eq!(
        restored_result.result_for_assistant.as_deref(),
        Some(assistant_output.as_str())
    );
    assert!(manager.get_session(&session_id).is_none());
}

#[test]
fn build_messages_from_turns_skips_model_invisible_turns() {
    use crate::service::session::{DialogTurnData, DialogTurnKind, UserMessageData};

    let turns = vec![
        DialogTurnData::new(
            "turn-1".to_string(),
            0,
            "session-1".to_string(),
            UserMessageData {
                id: "user-1".to_string(),
                content: "hello".to_string(),
                timestamp: 1,
                metadata: None,
            },
        ),
        DialogTurnData::new_with_kind(
            DialogTurnKind::ManualCompaction,
            "turn-2".to_string(),
            1,
            "session-1".to_string(),
            None,
            UserMessageData {
                id: "user-2".to_string(),
                content: "/compact".to_string(),
                timestamp: 2,
                metadata: None,
            },
        ),
        DialogTurnData::new_with_kind(
            DialogTurnKind::LocalCommand,
            "turn-3".to_string(),
            2,
            "session-1".to_string(),
            None,
            UserMessageData {
                id: "user-3".to_string(),
                content: "# Session Usage Report".to_string(),
                timestamp: 3,
                metadata: Some(serde_json::json!({
                    "localCommandKind": "usage_report",
                    "modelVisible": false
                })),
            },
        ),
    ];

    let messages = SessionManager::build_messages_from_turns(&turns);

    assert_eq!(messages.len(), 1);
    assert!(messages[0].is_actual_user_message());
}
