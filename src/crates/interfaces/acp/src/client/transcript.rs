//! Materialize the ACP event stream in the existing conversation history format.
//! The same reducer handles live delivery, journal recovery and agent replay.
use openbitfun_core::service::session::{
    DialogTurnData, ModelRoundData, TextItemData, ThinkingItemData, ToolCallData, ToolItemData,
    ToolResultData, TurnStatus, UserMessageData,
};
use openbitfun_events::{AgenticEvent, ToolEventData};
use serde_json::Value;

pub(super) fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

/// The visible message can differ from the text sent to the agent, which is what
/// an agent replays. Older records carry no such field and use the visible text.
pub(super) const PROMPT_TEXT_KEY: &str = "acpPromptText";

pub(super) fn prompt_text(turn: &DialogTurnData) -> &str {
    turn.user_message
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(PROMPT_TEXT_KEY))
        .and_then(Value::as_str)
        .unwrap_or(&turn.user_message.content)
}

pub(super) fn start_turn(
    session: &str,
    turn: &str,
    index: usize,
    client: &str,
    input: &str,
    metadata: Option<Value>,
) -> DialogTurnData {
    let user = UserMessageData {
        id: uuid::Uuid::new_v4().to_string(),
        content: input.to_owned(),
        timestamp: now_ms(),
        metadata,
    };
    let mut data = DialogTurnData::new(turn.into(), index, session.into(), user);
    data.agent_type = Some(format!("acp:{client}"));
    data
}

pub(super) fn start_round(turn: &mut DialogTurnData, id: &str, index: usize, timestamp: u64) {
    turn.model_rounds.push(ModelRoundData {
        id: id.into(),
        turn_id: turn.turn_id.clone(),
        round_index: index,
        round_group_id: None,
        timestamp,
        start_time: timestamp,
        end_time: None,
        duration_ms: None,
        text_items: Vec::new(),
        tool_items: Vec::new(),
        thinking_items: Vec::new(),
        provider_id: None,
        model_config_id: None,
        effective_model_name: None,
        routing: None,
        first_chunk_ms: None,
        first_visible_output_ms: None,
        stream_duration_ms: None,
        ttft_ms: None,
        attempt_count: None,
        attempt_diagnostics: Vec::new(),
        failure_category: None,
        token_details: None,
        status: "streaming".into(),
    });
}

pub(super) fn append_text(round: &mut ModelRoundData, text: &str, thinking: bool, time: u64) {
    let order = round.text_items.len() + round.thinking_items.len() + round.tool_items.len();
    if thinking {
        if let Some(last) = round
            .thinking_items
            .last_mut()
            .filter(|item| item.order_index == order.checked_sub(1))
        {
            last.content.push_str(text);
        } else {
            round.thinking_items.push(ThinkingItemData {
                id: format!("{}-thinking-{order}", round.id),
                content: text.into(),
                reasoning_kind: None,
                is_streaming: false,
                is_collapsed: true,
                timestamp: time,
                order_index: Some(order),
                status: None,
                is_subagent_item: None,
                parent_task_tool_id: None,
                subagent_session_id: None,
                attempt_id: None,
                attempt_index: None,
            });
        }
    } else if let Some(last) = round
        .text_items
        .last_mut()
        .filter(|item| item.order_index == order.checked_sub(1))
    {
        last.content.push_str(text);
    } else {
        round.text_items.push(TextItemData {
            id: format!("{}-text-{order}", round.id),
            content: text.into(),
            is_streaming: false,
            is_markdown: true,
            timestamp: time,
            order_index: Some(order),
            status: None,
            is_subagent_item: None,
            parent_task_tool_id: None,
            subagent_session_id: None,
            attempt_id: None,
            attempt_index: None,
        });
    }
}

pub(super) fn update_tool(round: &mut ModelRoundData, event: &ToolEventData, time: u64) {
    let identity = event.identity();
    let order = round.text_items.len() + round.thinking_items.len() + round.tool_items.len();
    let index = round
        .tool_items
        .iter()
        .position(|item| item.tool_call.id == identity.tool_id)
        .unwrap_or_else(|| {
            round.tool_items.push(ToolItemData {
                id: identity.tool_id.clone(),
                tool_name: identity.tool_name.clone(),
                tool_call: ToolCallData {
                    input: Value::Null,
                    id: identity.tool_id.clone(),
                },
                tool_result: None,
                ai_intent: None,
                start_time: time,
                end_time: None,
                duration_ms: None,
                queue_wait_ms: None,
                preflight_ms: None,
                confirmation_wait_ms: None,
                execution_ms: None,
                order_index: Some(order),
                is_subagent_item: None,
                parent_task_tool_id: None,
                subagent_session_id: None,
                subagent_dialog_turn_id: None,
                attempt_id: None,
                attempt_index: None,
                subagent_model_id: None,
                subagent_model_display_name: None,
                status: None,
                interruption_reason: None,
            });
            round.tool_items.len() - 1
        });
    let item = &mut round.tool_items[index];
    match event {
        ToolEventData::Started { params, .. }
        | ToolEventData::ConfirmationNeeded { params, .. } => {
            item.tool_call.input = params.clone();
            item.status = Some(
                if matches!(event, ToolEventData::ConfirmationNeeded { .. }) {
                    "confirming"
                } else {
                    "running"
                }
                .into(),
            );
        }
        ToolEventData::Completed {
            result,
            result_for_assistant,
            image_attachments,
            duration_ms,
            ..
        } => {
            item.tool_result = Some(ToolResultData {
                result: result.clone(),
                success: true,
                result_for_assistant: result_for_assistant.clone(),
                image_attachments: image_attachments.clone(),
                error: None,
                duration_ms: Some(*duration_ms),
            });
            item.duration_ms = Some(*duration_ms);
            item.end_time = Some(time);
            item.status = Some("completed".into());
        }
        ToolEventData::Failed {
            error, duration_ms, ..
        }
        | ToolEventData::Cancelled {
            reason: error,
            duration_ms,
            ..
        } => {
            item.tool_result = Some(ToolResultData {
                result: Value::String(error.clone()),
                success: false,
                result_for_assistant: None,
                image_attachments: None,
                error: Some(error.clone()),
                duration_ms: *duration_ms,
            });
            item.end_time = Some(time);
            item.status = Some(
                if matches!(event, ToolEventData::Cancelled { .. }) {
                    "cancelled"
                } else {
                    "error"
                }
                .into(),
            );
        }
        ToolEventData::Rejected { .. } => {
            item.status = Some("rejected".into());
            item.end_time = Some(time);
        }
        _ => {}
    }
}

pub(super) fn apply_event(turn: &mut DialogTurnData, event: &AgenticEvent, time: u64) {
    match event {
        AgenticEvent::ModelRoundStarted {
            round_id,
            round_index,
            ..
        } => {
            start_round(turn, round_id, *round_index, time);
        }
        AgenticEvent::TextChunk { text, .. } => {
            if let Some(round) = turn.model_rounds.last_mut() {
                append_text(round, text, false, time);
            }
        }
        AgenticEvent::ThinkingChunk { content, .. } => {
            if let Some(round) = turn.model_rounds.last_mut() {
                append_text(round, content, true, time);
            }
        }
        AgenticEvent::ToolEvent { tool_event, .. } => {
            if let Some(round) = turn.model_rounds.last_mut() {
                update_tool(round, tool_event, time);
            }
        }
        AgenticEvent::ModelRoundCompleted { .. } => {
            if let Some(round) = turn.model_rounds.last_mut() {
                round.status = "completed".into();
                round.end_time = Some(time);
                round.duration_ms = Some(time.saturating_sub(round.start_time));
            }
        }
        AgenticEvent::DialogTurnCompleted { .. } => turn.status = TurnStatus::Completed,
        AgenticEvent::DialogTurnCancelled { .. } => turn.status = TurnStatus::Cancelled,
        AgenticEvent::DialogTurnFailed {
            error,
            error_detail,
            ..
        } => {
            turn.status = TurnStatus::Error;
            turn.error = Some(error.clone());
            turn.error_detail = error_detail.clone();
            if error_detail
                .as_ref()
                .and_then(|d| d.recovery_constraint.as_deref())
                == Some("acp_stop_unconfirmed")
            {
                turn.finish_reason = Some("acp_outcome_unknown".into());
            }
        }
        _ => {}
    }
    if turn.status != TurnStatus::InProgress {
        for round in &mut turn.model_rounds {
            if round.status == "streaming" {
                round.status = match turn.status {
                    TurnStatus::Completed => "completed",
                    TurnStatus::Cancelled => "cancelled",
                    _ => "error",
                }
                .into();
                round.end_time = Some(time);
                round.duration_ms = Some(time.saturating_sub(round.start_time));
            }
            if matches!(turn.status, TurnStatus::Cancelled | TurnStatus::Error) {
                for tool in &mut round.tool_items {
                    if matches!(tool.status.as_deref(), Some("running" | "confirming")) {
                        tool.status = Some(
                            if turn.status == TurnStatus::Cancelled {
                                "cancelled"
                            } else {
                                "error"
                            }
                            .into(),
                        );
                        tool.end_time = Some(time);
                    }
                }
            }
        }
        turn.end_time = Some(time);
        turn.duration_ms = Some(time.saturating_sub(turn.start_time));
        turn.has_final_response = Some(
            turn.model_rounds
                .last()
                .is_some_and(|r| !r.text_items.is_empty()),
        );
    }
}
