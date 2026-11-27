//! A load response fences the history the agent sent before it. Register the
//! session receiver first and consume that history independently of live prompts.
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use agent_client_protocol::schema::{LoadSessionRequest, LoadSessionResponse};
use agent_client_protocol::{ActiveSession, Agent, ConnectionTo, Dispatch, Error, SessionMessage};
use futures::FutureExt;
use openbitfun_core::service::session::{DialogTurnData, ModelRoundData, TurnStatus};
use openbitfun_core::util::errors::OpenBitFunResult;

use super::super::session_persistence::{
    AcpSessionPersistence, PENDING_TURN_KEY, UNSENT_FINISH_REASON,
};
use super::super::stream::{
    decode_session_update, AcpClientStreamEvent, AcpStreamRoundTracker, AcpToolCallTracker,
};
use super::super::transcript;
use super::CLIENT_STARTUP_TIMEOUT;

#[derive(Default)]
pub(super) struct History {
    turns: Vec<DialogTurnData>,
    message_ids: Vec<Option<String>>,
    /// Byte ranges in the original user text. Without message IDs, adjacent
    /// chunks can belong to one prompt or to several unanswered prompts.
    runs: Vec<Vec<Range<usize>>>,
    /// The replay was requested to repair a known history gap.
    repair: bool,
    pub(super) metadata: Vec<AcpClientStreamEvent>,
    rounds: AcpStreamRoundTracker,
    tools: AcpToolCallTracker,
}

pub(super) async fn load(
    service: &super::AcpClientService,
    client: &std::sync::Arc<super::AcpClientConnection>,
    connection: &ConnectionTo<Agent>,
    remote_id: &str,
    cwd: &Path,
    active: &mut ActiveSession<'_, Agent>,
    session_id: &str,
    repair_history: bool,
) -> Result<(LoadSessionResponse, History), Error> {
    let response = connection
        .send_request(LoadSessionRequest::new(remote_id.to_owned(), cwd))
        .block_task();
    tokio::pin!(response);
    let mut history = History {
        repair: repair_history,
        ..History::default()
    };
    let mut finished = None;
    loop {
        let update = if finished.is_some() {
            // The router has received the response. Earlier notifications can
            // already be in this session's queue when both futures were ready.
            let Some(Ok(update)) = active.read_update().now_or_never() else {
                break;
            };
            update
        } else {
            tokio::select! {
                biased;
                result = &mut response => { finished = Some(result?); continue; }
                _ = tokio::time::sleep(CLIENT_STARTUP_TIMEOUT) => {
                    return Err(service.retire_timed_out_connection(client, "session/load", CLIENT_STARTUP_TIMEOUT).await);
                }
                update = active.read_update() => update?,
            }
        };
        if let SessionMessage::SessionMessage(dispatch) = update {
            // A load-only agent must replay even for a metadata read. Drain its
            // history without decoding messages or tool payloads the host already has.
            if !repair_history {
                let Dispatch::Notification(message) = &dispatch else {
                    continue;
                };
                if !matches!(
                    message
                        .params
                        .pointer("/update/sessionUpdate")
                        .and_then(|v| v.as_str()),
                    Some("available_commands_update" | "config_option_update" | "usage_update")
                ) {
                    continue;
                }
            }
            let (user, events) = decode_session_update(dispatch, &mut history.tools)
                .await
                .map_err(|error| agent_client_protocol::util::internal_error(error.to_string()))?;
            if let Some((id, text)) = user {
                history.user_chunk(session_id, &client.client_id, id, text.as_deref());
            }
            for event in events {
                history.push(event);
            }
        }
    }
    Ok((
        finished.expect("history is fenced by the load response"),
        history,
    ))
}

impl History {
    fn user_chunk(&mut self, session: &str, client: &str, id: Option<String>, text: Option<&str>) {
        let previous = self.message_ids.last();
        let same = id.is_some() && previous == Some(&id);
        let adjacent = id.is_none()
            && previous == Some(&None)
            && self.turns.last().is_some_and(|t| t.model_rounds.is_empty());
        if same || adjacent {
            if let (Some(turn), Some(run), Some(text)) =
                (self.turns.last_mut(), self.runs.last_mut(), text)
            {
                let content = &mut turn.user_message.content;
                let start = content.len();
                content.push_str(text);
                if same {
                    // An explicit message ID fixes the boundary of the whole prompt.
                    run[0].end = content.len();
                } else {
                    run.push(start..content.len());
                }
            }
            return;
        }
        let turn_id = uuid::Uuid::new_v4().to_string();
        let mut turn = transcript::start_turn(
            session,
            &turn_id,
            self.turns.len(),
            client,
            text.unwrap_or_default(),
            None,
        );
        if let Some(id) = &id {
            turn.user_message.id.clone_from(id);
        }
        self.message_ids.push(id);
        self.runs
            .push(std::iter::once(0..turn.user_message.content.len()).collect());
        self.turns.push(turn);
        self.tools = AcpToolCallTracker::default();
        self.rounds = AcpStreamRoundTracker::default();
    }

    fn push(&mut self, event: AcpClientStreamEvent) {
        if matches!(
            event,
            AcpClientStreamEvent::ConfigOptionsUpdated(_)
                | AcpClientStreamEvent::AvailableCommandsUpdated(_)
                | AcpClientStreamEvent::ContextUsageUpdated(_)
        ) {
            self.metadata
                .retain(|old| std::mem::discriminant(old) != std::mem::discriminant(&event));
            self.metadata.push(event);
            return;
        }
        let Some(turn) = self.turns.last_mut() else {
            return;
        };
        let time = turn.timestamp;
        for event in self.rounds.apply(event) {
            match event {
                AcpClientStreamEvent::ModelRoundStarted {
                    round_id,
                    round_index,
                    ..
                } => {
                    transcript::start_round(turn, &round_id, round_index, time);
                }
                AcpClientStreamEvent::AgentText(text) => {
                    if let Some(round) = turn.model_rounds.last_mut() {
                        transcript::append_text(round, &text, false, time);
                    }
                }
                AcpClientStreamEvent::AgentThought(text) => {
                    if let Some(round) = turn.model_rounds.last_mut() {
                        transcript::append_text(round, &text, true, time);
                    }
                }
                AcpClientStreamEvent::ToolEvent(event) => {
                    if let Some(round) = turn.model_rounds.last_mut() {
                        transcript::update_tool(round, &event, time);
                    }
                }
                _ => {}
            }
        }
    }

    /// Local history can start at any chunk boundary in an unidentified run.
    /// Keep only the first complete placement. A second one already proves
    /// that position cannot identify the replies.
    fn unique_placement(
        &self,
        saved: &[&DialogTurnData],
        id_matches: &impl Fn(&DialogTurnData, usize) -> bool,
    ) -> Result<(usize, usize), Outcome> {
        if saved.is_empty() {
            return Err(Outcome::Ambiguous);
        }
        let open_end = self
            .turns
            .last()
            .is_some_and(|last| last.model_rounds.is_empty());
        let mut first = None;
        let mut incomplete = false;
        for (run, chunks) in self.runs.iter().enumerate() {
            for offset in 0..chunks.len() {
                match self.match_from((run, offset), saved, id_matches, |_, _, _| {}) {
                    Alignment::Complete { .. } if first.is_some() => {
                        return Err(if open_end {
                            Outcome::Retry
                        } else {
                            Outcome::Ambiguous
                        });
                    }
                    Alignment::Complete { at_replay_end } => {
                        first = Some(((run, offset), at_replay_end));
                    }
                    Alignment::Incomplete => incomplete = true,
                    Alignment::Mismatch => {}
                }
            }
        }
        match first {
            // A later, unfinished placement can still own the saved prompts if
            // the complete match leaves part of this replay unexplained.
            Some((_, at_end)) if incomplete && (open_end || !at_end) => Err(Outcome::Retry),
            Some((position, _)) => Ok(position),
            None if incomplete => Err(Outcome::Retry),
            None => Err(Outcome::Ambiguous),
        }
    }

    /// Walk the saved prompts without allocating per candidate. Only the last
    /// prompt in a fully consumed run can own the reply that follows it.
    fn match_from(
        &self,
        (start, offset): (usize, usize),
        saved: &[&DialogTurnData],
        id_matches: &impl Fn(&DialogTurnData, usize) -> bool,
        mut matched: impl FnMut(usize, Range<usize>, bool),
    ) -> Alignment {
        let mut next = 0;
        for index in start..self.runs.len() {
            let run = &self.runs[index];
            let replayed = &self.turns[index].user_message.content;
            let from = if index == start { offset } else { 0 };
            let first = next;
            let mut used = 0;
            let mut at_boundary = false;
            for (i, range) in run.iter().enumerate().skip(from) {
                let chunk = &replayed[range.clone()];
                let prompt = transcript::prompt_text(saved[next]);
                if !id_matches(saved[next], index)
                    || (prompt.is_empty() && run.len() > 1)
                    || !prompt[used..].starts_with(chunk)
                {
                    return Alignment::Mismatch;
                }
                used += chunk.len();
                at_boundary = used == prompt.len();
                if at_boundary {
                    next += 1;
                    used = 0;
                    if next == saved.len() {
                        matched(index, first..next, i + 1 == run.len());
                        return Alignment::Complete {
                            at_replay_end: index + 1 == self.runs.len() && i + 1 == run.len(),
                        };
                    }
                }
            }
            if !at_boundary {
                // A reply or a new run closes the previous prompt boundary.
                return if index + 1 == self.runs.len() && self.turns[index].model_rounds.is_empty()
                {
                    Alignment::Incomplete
                } else {
                    Alignment::Mismatch
                };
            }
            matched(index, first..next, true);
        }
        // The replay ended exactly between two saved prompts.
        Alignment::Incomplete
    }

    pub(super) async fn reconcile(
        self,
        store: &AcpSessionPersistence,
        path: &Path,
        session: &str,
    ) -> OpenBitFunResult<()> {
        let outcome = self.attempt(store, path, session).await?;
        // Only a boundary the agent's history cannot settle is remembered. An
        // empty, short or still-growing replay can succeed on a later attempt.
        if self.repair && outcome == Outcome::Ambiguous {
            store.record_repair_attempt(path, session).await?;
        }
        Ok(())
    }

    async fn attempt(
        &self,
        store: &AcpSessionPersistence,
        path: &Path,
        session: &str,
    ) -> OpenBitFunResult<Outcome> {
        if self.turns.is_empty() {
            return Ok(Outcome::Retry);
        }
        let Some(record) = store.manager.load_session_metadata(path, session).await? else {
            return Ok(Outcome::Repaired);
        };
        if !AcpSessionPersistence::has_history_gap(&record) {
            return Ok(Outcome::Repaired);
        }
        let custom = record.custom_metadata.as_ref();
        let first_owned = custom.and_then(|v| v.get("acpRuntimeFromTurn")?.as_u64());
        let pending_id = custom.and_then(|v| v.get(PENDING_TURN_KEY)?.get("turnId")?.as_str());
        let mut local = store
            .manager
            .load_visible_session_turns(path, session)
            .await?;
        // An older view can have checkpointed the incoming optimistic turn.
        // Its admission is already durable and the agent has not seen it yet.
        if local
            .last()
            .is_some_and(|turn| Some(turn.turn_id.as_str()) == pending_id)
        {
            local.pop();
        }
        let locals: Vec<_> = local
            .iter()
            .filter(|t| {
                t.kind == openbitfun_core::service::session::DialogTurnKind::UserDialog
                    && t.finish_reason.as_deref() != Some(UNSENT_FINISH_REASON)
            })
            .collect();
        // An unsent local turn has no position in the agent's history. Appending a remote
        // suffix after that receipt would invent an order, and reusing its index would overwrite it.
        let trailing_unsent = local
            .iter()
            .rev()
            .find(|t| t.kind == openbitfun_core::service::session::DialogTurnKind::UserDialog)
            .is_some_and(|t| t.finish_reason.as_deref() == Some(UNSENT_FINISH_REASON));
        let replay_index_by_id: HashMap<_, _> = self
            .message_ids
            .iter()
            .enumerate()
            .filter_map(|(index, id)| Some((id.as_deref()?, index)))
            .collect();
        let id_matches = |local: &DialogTurnData, index: usize| match replay_index_by_id
            .get(local.user_message.id.as_str())
        {
            Some(&identified) => identified == index,
            None => {
                self.message_ids[index].is_none()
                    || !first_owned.is_some_and(|first| local.turn_index as u64 >= first)
            }
        };
        let mut by_position = vec![None; self.turns.len()];
        let mut append_from = None;
        let mut matched = 0;
        let mut complete = true;
        let mut unresolved = Outcome::Retry;
        if self.runs.iter().any(|run| run.len() > 1) {
            match self.unique_placement(&locals, &id_matches) {
                Ok(start) => {
                    complete = start == (0, 0);
                    let mut end = start.0;
                    let mut ends_run = false;
                    self.match_from(start, &locals, &id_matches, |index, group, at_end| {
                        end = index + 1;
                        ends_run = at_end;
                        let unanswered = group.start..group.end - usize::from(at_end);
                        matched += unanswered.len();
                        complete &= locals[unanswered]
                            .iter()
                            .all(|t| t.status == TurnStatus::Completed);
                        if at_end {
                            by_position[index] = Some(group.end - 1);
                        }
                    });
                    if ends_run && self.runs[end..].iter().all(|run| run.len() == 1) {
                        append_from = Some(end);
                    } else {
                        // There is no saved prompt to decide the remaining chunk boundaries.
                        complete = false;
                        unresolved = Outcome::Ambiguous;
                    }
                }
                Err(outcome) => {
                    if replay_index_by_id.is_empty() {
                        return Ok(outcome);
                    }
                    // Explicit message IDs can still repair their own turns.
                    complete = false;
                    unresolved = outcome;
                }
            }
        } else {
            let same_prompt = |local: &DialogTurnData, index: usize| {
                transcript::prompt_text(local) == self.turns[index].user_message.content
                    && id_matches(local, index)
            };
            let aligned = self.turns.len() >= locals.len()
                && locals
                    .iter()
                    .enumerate()
                    .all(|(i, turn)| same_prompt(turn, i));
            let repeated_tail = aligned
                && locals.last().is_some_and(|last| {
                    (locals.len()..self.turns.len()).any(|i| same_prompt(last, i))
                });
            // A later copy of the local prefix may be the part still being
            // replayed. It can start at most one local history's length back.
            let last = self.turns.len() - 1;
            let partial_tail = aligned
                && !locals.is_empty()
                && self.turns.len() > locals.len()
                && (self.turns.len() - locals.len()..self.turns.len())
                    .rev()
                    .any(|start| {
                        (self.turns[last].model_rounds.is_empty()
                            || same_prompt(locals[last - start], last))
                            && matches!(
                                self.match_from((start, 0), &locals, &id_matches, |_, _, _| {}),
                                Alignment::Incomplete
                            )
                    });
            if partial_tail {
                complete = false;
            } else if aligned && !repeated_tail {
                for (i, owner) in by_position[..locals.len()].iter_mut().enumerate() {
                    *owner = Some(i);
                }
                append_from = Some(locals.len());
            } else {
                if repeated_tail {
                    unresolved = Outcome::Ambiguous;
                }
                // Outside an aligned prefix, use only text unique on both sides.
                let mut unique_text = HashMap::new();
                for (i, remote) in self.turns.iter().enumerate() {
                    let text = remote.user_message.content.as_str();
                    if !text.is_empty() {
                        unique_text
                            .entry(text)
                            .and_modify(|index| *index = None)
                            .or_insert(Some(i));
                    }
                }
                let mut by_text = HashMap::new();
                for (i, local) in locals.iter().enumerate() {
                    if let Some(&Some(remote)) = unique_text.get(transcript::prompt_text(local)) {
                        if same_prompt(local, remote) {
                            by_text
                                .entry(remote)
                                .and_modify(|index| *index = None)
                                .or_insert(Some(i));
                        }
                    }
                }
                for (remote, owner) in by_text {
                    by_position[remote] = owner;
                }
            }
        }
        let mut local_by_id = HashMap::new();
        for (i, turn) in locals.iter().enumerate() {
            local_by_id
                .entry(turn.user_message.id.as_str())
                .or_insert(i);
        }
        // The visible catalog can end before turn files that a lagging metadata
        // count does not list yet. Import after every stored file.
        let mut next_index = store
            .next_free_index(path, session, local.last().map_or(0, |t| t.turn_index + 1))
            .await?;
        let mut last_local = None;
        let mut written = false;
        for (ordinal, (remote, id)) in self.turns.iter().zip(&self.message_ids).enumerate() {
            let found = id
                .as_ref()
                .and_then(|id| local_by_id.get(id.as_str()).copied())
                .or(by_position[ordinal]);
            if let Some(i) = found {
                if last_local.is_some_and(|last| i <= last) {
                    complete = false;
                    continue;
                }
                last_local = Some(i);
                matched += 1;
                let previous = locals[i];
                if previous.status == TurnStatus::InProgress {
                    complete = false;
                    continue;
                }
                if previous.status == TurnStatus::Completed {
                    continue;
                }
                let mut repaired = previous.clone();
                if !merge_rounds(&mut repaired, &remote.model_rounds) {
                    complete = false;
                    continue;
                }
                if remote.model_rounds.is_empty() {
                    complete = false;
                    continue;
                }
                if serde_json::to_value(&repaired)? != serde_json::to_value(previous)? {
                    store.manager.save_dialog_turn(path, &repaired).await?;
                    written = true;
                }
            } else if append_from.is_some_and(|start| ordinal >= start)
                && !trailing_unsent
                && matched == locals.len()
            {
                let mut imported = remote.clone();
                imported.turn_index = next_index;
                // ACP v1 replays items, not the previous prompt response. Retain
                // that distinction even when the replay contains assistant text.
                imported.status = TurnStatus::Error;
                imported.finish_reason = Some("acp_outcome_unknown".into());
                imported.error =
                    Some("ACP history was restored without a confirmed turn outcome.".into());
                for round in &mut imported.model_rounds {
                    round.status = "completed".into();
                }
                store.manager.save_dialog_turn(path, &imported).await?;
                next_index += 1;
                written = true;
            } else {
                complete = false;
            }
        }
        if complete && matched == locals.len() {
            store.history_repaired(path, session).await?;
            return Ok(Outcome::Repaired);
        }
        if written || !complete {
            log::info!("Preserved local ACP history after partial replay: session_id={session}");
        }
        Ok(unresolved)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// No gap remains.
    Repaired,
    /// Prompt boundaries cannot be told apart from the replay and the saved prompts.
    Ambiguous,
    /// Anything else may resolve once the agent's history is complete.
    Retry,
}

enum Alignment {
    Complete { at_replay_end: bool },
    Incomplete,
    Mismatch,
}

/// Apply only additions supported by the replay. Conflicting or shorter
/// history leaves the original record intact, including item IDs and timing.
pub(super) fn merge_rounds(turn: &mut DialogTurnData, incoming: &[ModelRoundData]) -> bool {
    if incoming.len() < turn.model_rounds.len() {
        return false;
    }
    for (index, fresh) in incoming.iter().enumerate() {
        let Some(old) = turn.model_rounds.get_mut(index) else {
            let mut round = fresh.clone();
            round.turn_id.clone_from(&turn.turn_id);
            round.status = "completed".into();
            turn.model_rounds.push(round);
            continue;
        };
        if old.text_items.len() > fresh.text_items.len()
            || old.thinking_items.len() > fresh.thinking_items.len()
        {
            return false;
        }
        for (item, next) in old.text_items.iter_mut().zip(&fresh.text_items) {
            if !next.content.starts_with(&item.content)
                || item
                    .order_index
                    .zip(next.order_index)
                    .is_some_and(|(a, b)| a != b)
            {
                return false;
            }
            item.content.clone_from(&next.content);
        }
        for (item, next) in old.thinking_items.iter_mut().zip(&fresh.thinking_items) {
            if !next.content.starts_with(&item.content)
                || item
                    .order_index
                    .zip(next.order_index)
                    .is_some_and(|(a, b)| a != b)
            {
                return false;
            }
            item.content.clone_from(&next.content);
        }
        for item in &mut old.tool_items {
            let Some(next) = fresh
                .tool_items
                .iter()
                .find(|v| v.tool_call.id == item.tool_call.id)
            else {
                return false;
            };
            if item.tool_result.is_none() && next.tool_result.is_some() {
                item.tool_result.clone_from(&next.tool_result);
                item.status.clone_from(&next.status);
            }
        }
        old.text_items
            .extend_from_slice(&fresh.text_items[old.text_items.len()..]);
        old.thinking_items
            .extend_from_slice(&fresh.thinking_items[old.thinking_items.len()..]);
        for tool in &fresh.tool_items {
            if !old
                .tool_items
                .iter()
                .any(|t| t.tool_call.id == tool.tool_call.id)
            {
                old.tool_items.push(tool.clone());
            }
        }
    }
    true
}
