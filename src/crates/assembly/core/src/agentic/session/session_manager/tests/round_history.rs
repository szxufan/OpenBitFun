use super::super::SessionManager;
use super::support::{test_manager, TestWorkspace};
use crate::agentic::core::{Message, SessionConfig, ToolCall, ToolResult, TurnStats};
use crate::agentic::persistence::PersistenceManager;
use crate::service::session::{DialogTurnData, TurnStatus, UserMessageData};
use serde_json::json;
use std::sync::Arc;

#[test]
fn model_routing_history_survives_headless_persistence_and_legacy_messages() {
    let legacy = Message::assistant("legacy".into());
    let mut legacy_json = serde_json::to_value(&legacy).unwrap();
    legacy_json["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("model_execution");
    let restored: Message = serde_json::from_value(legacy_json).unwrap();
    assert!(restored.metadata.model_execution.is_none());
    let mut routed = Message::assistant("routed".into()).with_round_id("round-economy".into());
    routed.metadata.model_execution = Some(crate::agentic::core::message::ModelExecutionMetadata {
        model_config_id: "economy-config".into(),
        effective_model_name: "economy-api-model".into(),
        routing: Some(openbitfun_events::agentic::ModelRoutingDecision {
            route: "economy".into(),
            latency_ms: 17,
            fallback_reason: None,
        }),
        timing: None,
    });
    let restored: Message = serde_json::from_value(serde_json::to_value(routed).unwrap()).unwrap();
    let rounds = SessionManager::build_model_rounds_from_messages(&[legacy, restored], "turn", 1);
    assert!(rounds[0].routing.is_none());
    assert_eq!(rounds[1].model_config_id.as_deref(), Some("economy-config"));
    assert_eq!(
        rounds[1].effective_model_name.as_deref(),
        Some("economy-api-model")
    );
    assert_eq!(rounds[1].routing.as_ref().unwrap().latency_ms, 17);
}

#[test]
fn completion_merge_keeps_runtime_round_timing_over_stale_client_checkpoint() {
    let timed_message = |round_id: &str, timing| {
        let mut message = Message::assistant("answer".into()).with_round_id(round_id.into());
        message.metadata.model_execution =
            Some(crate::agentic::core::message::ModelExecutionMetadata {
                model_config_id: "model".into(),
                effective_model_name: "model".into(),
                routing: None,
                timing,
            });
        message
    };
    let mut turn = DialogTurnData::new(
        "turn".into(),
        0,
        "session".into(),
        UserMessageData {
            id: "input".into(),
            content: "hi".into(),
            timestamp: 1,
            metadata: None,
        },
    );
    // The client checkpointed the final round before its
    // `ModelRoundCompleted` arrived, and a checkpoint from an older writer
    // recorded an unknown duration as 0.
    let mut stale = SessionManager::build_model_rounds_from_messages(
        &[timed_message("final", None)],
        "turn",
        1,
    );
    stale[0].duration_ms = Some(0);
    // A legacy journal without timing keeps the client's real facts.
    let mut checkpointed = SessionManager::build_model_rounds_from_messages(
        &[timed_message("legacy", None)],
        "turn",
        1,
    );
    checkpointed[0].duration_ms = Some(900);
    checkpointed[0].ttft_ms = Some(150);
    turn.model_rounds = vec![checkpointed.remove(0), stale.remove(0)];

    let runtime_timing = crate::agentic::core::message::ModelRoundTimingMetadata {
        duration_ms: Some(2172),
        first_chunk_ms: Some(10),
        first_visible_output_ms: Some(20),
        stream_duration_ms: Some(1500),
        ttft_ms: Some(690),
        attempt_count: Some(1),
    };
    SessionManager::append_generation_rounds(
        &mut turn,
        "turn",
        &[
            timed_message("legacy", None),
            timed_message("final", Some(runtime_timing)),
        ],
        2,
    );

    let legacy = &turn.model_rounds[0];
    assert_eq!(legacy.duration_ms, Some(900));
    assert_eq!(legacy.ttft_ms, Some(150));
    let last = &turn.model_rounds[1];
    assert_eq!(last.duration_ms, Some(2172));
    assert_eq!(last.ttft_ms, Some(690));
    assert_eq!(last.stream_duration_ms, Some(1500));
    assert_eq!(last.attempt_count, Some(1));
}

#[test]
fn generation_round_order_follows_journal_for_sparse_checkpoints() {
    let ids = ["z-opening", "b-tool", "y-analysis", "a-tool", "m-final"];
    let messages = ids
        .iter()
        .map(|id| Message::assistant((*id).into()).with_round_id((*id).into()))
        .collect::<Vec<_>>();
    // Runtime-rebuilt rounds can all have the completion timestamp. Neither
    // timestamps, lexical IDs nor a partial checkpoint's indexes own order.
    let rounds = SessionManager::build_model_rounds_from_messages(&messages, "turn", 10);
    for mask in 0..(1 << ids.len()) {
        for reverse in [false, true] {
            let mut turn = DialogTurnData::new(
                "turn".into(),
                0,
                "session".into(),
                UserMessageData {
                    id: "input".into(),
                    content: "request".into(),
                    timestamp: 1,
                    metadata: None,
                },
            );
            turn.model_rounds = rounds
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, round)| round.clone())
                .collect();
            if reverse {
                turn.model_rounds.reverse();
            }
            for (index, round) in turn.model_rounds.iter_mut().enumerate() {
                round.round_index = index;
                round.provider_id = Some("checkpoint-provider".into());
            }
            // The saved legacy shape needs no new fields to recover order.
            let mut turn: DialogTurnData =
                serde_json::from_value(serde_json::to_value(turn).unwrap()).unwrap();
            for _ in 0..2 {
                assert_eq!(
                    SessionManager::append_generation_rounds(&mut turn, "turn", &messages, 10),
                    ids.len()
                );
                assert_eq!(
                    turn.model_rounds
                        .iter()
                        .map(|round| round.id.as_str())
                        .collect::<Vec<_>>(),
                    ids,
                    "mask={mask}, reverse={reverse}"
                );
                for (index, round) in turn.model_rounds.iter().enumerate() {
                    assert_eq!(round.round_index, index);
                    assert_eq!(round.text_items[0].content, ids[index]);
                    if mask & (1 << index) != 0 {
                        assert_eq!(round.provider_id.as_deref(), Some("checkpoint-provider"));
                    }
                }
            }
        }
    }
}

#[test]
fn generation_round_order_preserves_prior_generations_and_unjournaled_rounds() {
    let message = |id: &str| Message::assistant(id.into()).with_round_id(id.into());
    let mut turn = DialogTurnData::new(
        "turn".into(),
        0,
        "session".into(),
        UserMessageData {
            id: "input".into(),
            content: "request".into(),
            timestamp: 1,
            metadata: None,
        },
    );
    turn.model_rounds = SessionManager::build_model_rounds_from_messages(
        &[
            message("interrupted"),
            message("second"),
            message("retry"),
            message("final"),
            message("pending"),
        ],
        "turn",
        10,
    );
    turn.model_rounds[0].status = "cancelled".into();
    turn.model_rounds[2].status = "error".into();
    turn.model_rounds[4].status = "streaming".into();
    let preserved = [0, 2, 4].map(|index| turn.model_rounds[index].clone());
    let messages = [message("first"), message("second"), message("final")];
    for _ in 0..2 {
        SessionManager::append_generation_rounds(&mut turn, "turn", &messages, 20);
        assert_eq!(
            turn.model_rounds
                .iter()
                .map(|round| round.id.as_str())
                .collect::<Vec<_>>(),
            [
                "interrupted",
                "first",
                "second",
                "retry",
                "final",
                "pending"
            ]
        );
        for (old, index) in preserved.iter().zip([0, 3, 5]) {
            let mut expected = old.clone();
            expected.round_index = index;
            assert_eq!(
                serde_json::to_value(&turn.model_rounds[index]).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
        }
    }
    let before = serde_json::to_value(&turn).unwrap();
    assert_eq!(
        SessionManager::append_generation_rounds(&mut turn, "turn", &[], 30),
        0
    );
    assert_eq!(serde_json::to_value(&turn).unwrap(), before);
}

#[test]
fn classified_edit_history_preserves_detail_and_readable_error() {
    let assistant = Message::assistant_with_tools(
        String::new(),
        vec![ToolCall {
            tool_id: "edit-1".into(),
            tool_name: "Edit".into(),
            arguments: json!({}),
            raw_arguments: None,
            is_error: false,
            parse_error: None,
            recovered_from_truncation: false,
            repair_kind: Default::default(),
        }],
    );
    let result = Message::tool_result(ToolResult {
        tool_id: "edit-1".into(),
        tool_name: "Edit".into(),
        effective_tool_name: None,
        result: json!({"error":"[guidance] Inputs are equal", "error_detail":{"code":"edit_no_change", "kind":"guidance"}}),
        result_for_assistant: None,
        is_error: true,
        duration_ms: None,
        image_attachments: None,
    });
    let rounds =
        SessionManager::build_model_rounds_from_messages(&[assistant, result], "turn-1", 1);
    let encoded = serde_json::to_value(&rounds[0]).unwrap();
    let restored: crate::service::session::ModelRoundData =
        serde_json::from_value(encoded).unwrap();
    let result = restored.tool_items[0].tool_result.as_ref().unwrap();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("[guidance] Inputs are equal"));
    assert_eq!(result.result["error_detail"]["code"], "edit_no_change");
}

#[test]
fn persisted_round_preserves_deferred_wire_call_and_effective_identity() {
    let assistant = Message::assistant_with_tools(
        String::new(),
        vec![ToolCall {
            tool_id: "tool-1".to_string(),
            tool_name: openbitfun_agent_tools::CALL_DEFERRED_TOOL_NAME.to_string(),
            arguments: json!({
                "tool_name": "WebFetch",
                "args": { "url": "https://example.test" }
            }),
            raw_arguments: None,
            is_error: false,
            parse_error: None,
            recovered_from_truncation: false,
            repair_kind: Default::default(),
        }],
    )
    .with_turn_id("turn-1".to_string())
    .with_round_id("round-1".to_string());
    let result = Message::tool_result(ToolResult {
        tool_id: "tool-1".to_string(),
        tool_name: openbitfun_agent_tools::CALL_DEFERRED_TOOL_NAME.to_string(),
        effective_tool_name: Some("WebFetch".to_string()),
        result: json!({ "content": "external content" }),
        result_for_assistant: Some("external content".to_string()),
        is_error: false,
        duration_ms: Some(1),
        image_attachments: None,
    })
    .with_turn_id("turn-1".to_string())
    .with_round_id("round-1".to_string());

    let persisted_messages: Vec<Message> = serde_json::from_value(
        serde_json::to_value(vec![assistant, result]).expect("serialize messages"),
    )
    .expect("deserialize messages");
    let provider_result: crate::util::types::Message = (&persisted_messages[1]).into();
    assert_eq!(
        provider_result.name.as_deref(),
        Some(openbitfun_agent_tools::CALL_DEFERRED_TOOL_NAME)
    );

    let rounds = SessionManager::build_model_rounds_from_messages(&persisted_messages, "turn-1", 1);

    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].tool_items.len(), 1);
    let tool = &rounds[0].tool_items[0];
    assert_eq!(
        tool.tool_name,
        openbitfun_agent_tools::CALL_DEFERRED_TOOL_NAME
    );
    assert_eq!(
        tool.tool_call.input,
        json!({
            "tool_name": "WebFetch",
            "args": { "url": "https://example.test" }
        })
    );
    let (effective_name, effective_input) = crate::service::session::effective_tool_identity(tool);
    assert_eq!(effective_name, "WebFetch");
    assert_eq!(effective_input, &json!({ "url": "https://example.test" }));
}

#[tokio::test]
async fn completion_turn_usage_survives_missing_or_stale_client_and_reopen() {
    use crate::service::token_usage::TokenUsageService;
    for checkpointed in [false, true] {
        let workspace = TestWorkspace::new();
        let persistence = Arc::new(PersistenceManager::new(workspace.path_manager()).unwrap());
        let manager = test_manager(persistence.clone());
        let session = manager
            .create_session(
                "Turn usage".into(),
                "Standard".into(),
                SessionConfig {
                    workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let turn_id = manager
            .start_dialog_turn(
                &session.session_id,
                "Standard".into(),
                "weather".into(),
                Some("turn-usage".into()),
                None,
                None,
            )
            .await
            .unwrap();
        if checkpointed {
            let mut turn = persistence
                .load_dialog_turn(workspace.path(), &session.session_id, 0)
                .await
                .unwrap()
                .unwrap();
            // An older frontend saved only the first request before the
            // final request completed. The completion must replace it.
            turn.token_usage = Some(
                serde_json::from_value(json!({
                    "input_tokens": 28928, "output_tokens": 97,
                    "total_tokens": 29025, "timestamp": 1
                }))
                .unwrap(),
            );
            persistence
                .save_dialog_turn(workspace.path(), &turn)
                .await
                .unwrap();
        }
        let ledger = TokenUsageService::new(workspace.path_manager())
            .await
            .unwrap();
        let details = |attempt: &str| {
            Some(json!({"openbitfunModelRequest": {
                "version": 1, "roundId": attempt, "attemptId": attempt,
                "attemptIndex": 1, "startedAt": chrono::Utc::now(),
                "authentication": "api_key", "requestedModel": "model",
                "outcome": "completed", "usageKnown": true
            }}))
        };
        let second_details = details("second");
        for (sid, tid, input, output, detail, is_child) in [
            (
                session.session_id.as_str(),
                turn_id.as_str(),
                28928,
                97,
                details("first"),
                false,
            ),
            (
                session.session_id.as_str(),
                turn_id.as_str(),
                42794,
                237,
                second_details.clone(),
                false,
            ),
            (
                session.session_id.as_str(),
                turn_id.as_str(),
                6000,
                600,
                None,
                false,
            ),
            // Subscriber replay of the same immutable attempt is deduped.
            (
                session.session_id.as_str(),
                turn_id.as_str(),
                42794,
                237,
                second_details,
                false,
            ),
            (
                session.session_id.as_str(),
                "other-turn",
                9000,
                900,
                details("other"),
                false,
            ),
            (
                "child-session",
                turn_id.as_str(),
                8000,
                800,
                details("child"),
                true,
            ),
        ] {
            ledger
                .record_usage(
                    "config".into(),
                    "model".into(),
                    sid.into(),
                    tid.into(),
                    input,
                    output,
                    None,
                    detail,
                    is_child,
                )
                .await
                .unwrap();
        }
        for _ in 0..2 {
            manager
                .complete_dialog_turn(
                    &session.session_id,
                    &turn_id,
                    "Sunny".into(),
                    &[],
                    TurnStats {
                        total_rounds: 2,
                        total_tools: 1,
                        total_tokens: 0,
                        duration_ms: 4250,
                    },
                    Some("complete".into()),
                    Some(true),
                )
                .await
                .unwrap();
        }
        // Read through a fresh persistence instance, with no live client
        // or event journal available to fill in the missing final usage.
        let reopened = PersistenceManager::new(workspace.path_manager()).unwrap();
        let restored = reopened
            .load_dialog_turn(workspace.path(), &session.session_id, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.status, TurnStatus::Completed);
        let usage = restored.token_usage.unwrap();
        assert_eq!(usage.input_tokens, 71722);
        assert_eq!(usage.output_tokens, Some(334));
        assert_eq!(usage.total_tokens, 72056);
        assert_eq!(usage.cached_tokens, None);
        assert!((usage.output_tokens.unwrap() as f64 / 4.25 - 78.588235).abs() < 0.00001);
    }
}

#[tokio::test]
async fn generation_round_order_survives_completion_with_a_projected_final_prefix() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Completion merge".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "finish the response".to_string(),
            Some("turn-completion-prefix".to_string()),
            None,
            None,
        )
        .await
        .expect("turn should start");

    let projected_prefix = Message::assistant("Saved prefix: 1.".to_string())
        .with_turn_id(turn_id.clone())
        .with_round_id("round-final".to_string());
    let mut persisted_turn = persistence_manager
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .expect("turn should load")
        .expect("turn should exist");
    persisted_turn.model_rounds =
        SessionManager::build_model_rounds_from_messages(&[projected_prefix], &turn_id, 1);
    persisted_turn.model_rounds[0].tool_items.push(
        serde_json::from_value(serde_json::json!({
            "id": "plan-display-test",
            "toolName": "CreatePlan",
            "toolCall": { "id": "", "input": {} },
            "toolResult": {
                "result": { "plan_file_path": "/tmp/plan.md" },
                "success": true
            },
            "startTime": 1,
            "status": "completed"
        }))
        .expect("derived plan display tool"),
    );
    persistence_manager
        .save_dialog_turn(workspace.path(), &persisted_turn)
        .await
        .expect("projected prefix should persist");

    let complete_response = "Saved prefix: 1. first item\n2. second item";
    let earlier = Message::assistant_with_tools(
        "Checking the source".into(),
        vec![ToolCall {
            tool_id: "read-source".into(),
            tool_name: "Read".into(),
            arguments: json!({"path": "README.md"}),
            raw_arguments: None,
            is_error: false,
            parse_error: None,
            recovered_from_truncation: false,
            repair_kind: Default::default(),
        }],
    )
    .with_turn_id(turn_id.clone())
    .with_round_id("round-first".into());
    let result = Message::tool_result(ToolResult {
        tool_id: "read-source".into(),
        tool_name: "Read".into(),
        effective_tool_name: None,
        result: json!({"content": "source"}),
        result_for_assistant: None,
        is_error: false,
        duration_ms: Some(1),
        image_attachments: None,
    })
    .with_turn_id(turn_id.clone())
    .with_round_id("round-first".into());
    manager
        .complete_dialog_turn(
            &session.session_id,
            &turn_id,
            complete_response.to_string(),
            &[
                earlier,
                result,
                Message::assistant(complete_response.to_string())
                    .with_turn_id(turn_id.clone())
                    .with_round_id("round-final".to_string()),
            ],
            TurnStats {
                total_rounds: 2,
                total_tools: 1,
                total_tokens: 0,
                duration_ms: 1,
            },
            Some("complete".to_string()),
            Some(true),
        )
        .await
        .expect("completion should persist");

    let completed = persistence_manager
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .expect("turn should load")
        .expect("turn should exist");
    assert_eq!(completed.status, TurnStatus::Completed);
    assert_eq!(completed.finish_reason.as_deref(), Some("complete"));
    assert_eq!(completed.has_final_response, Some(true));
    assert_eq!(completed.model_rounds.len(), 2);
    assert_eq!(completed.model_rounds[0].id, "round-first");
    assert_eq!(completed.model_rounds[0].round_index, 0);
    assert!(
        completed.model_rounds[0].tool_items[0]
            .tool_result
            .as_ref()
            .unwrap()
            .success
    );
    assert_eq!(completed.model_rounds[1].id, "round-final");
    assert_eq!(completed.model_rounds[1].round_index, 1);
    assert_eq!(
        completed.model_rounds[1].text_items[0].content,
        complete_response,
    );
    assert_eq!(completed.model_rounds[1].tool_items.len(), 1);
    assert_eq!(
        completed.model_rounds[1].tool_items[0].id,
        "plan-display-test",
    );
}
