//! Read existing runtime/recovery facts without restoring or mutating a session.
use super::*;

pub(crate) struct ContextEstimateInputs {
    pub session: Session,
    pub messages: Vec<Message>,
    pub prompt_cache: SessionPromptCache,
    pub cache_ttl: Option<std::time::Duration>,
    pub listing_baseline: Option<TurnSkillAgentSnapshot>,
    pub turn_id: String,
    pub turn_metadata: Option<serde_json::Value>,
    pub turn_index: usize,
}

impl SessionManager {
    /// Caller holds the session mutation permit and has validated storage ownership.
    pub(crate) async fn read_context_estimate_inputs(
        &self,
        storage: &Path,
        session_id: &str,
    ) -> OpenBitFunResult<ContextEstimateInputs> {
        let persistence = &self.persistence_manager;
        let session = match self.get_session(session_id) {
            Some(session) => session,
            None => persistence.load_session_header(storage, session_id).await?,
        };
        let boundary = persistence
            .load_session_revert_state(storage, session_id)
            .await?
            .map(|state| state.boundary_turn);
        let mut turns = persistence.load_session_turns(storage, session_id).await?;
        turns.retain(|turn| boundary.is_none_or(|end| turn.turn_index < end));
        let turn_id = turns
            .last()
            .map(|turn| turn.turn_id.clone())
            .unwrap_or_default();
        let turn_metadata = turns
            .last()
            .and_then(|turn| turn.user_message.metadata.clone());
        let turn_index = turns.last().map(|turn| turn.turn_index).unwrap_or(0);
        let messages = if boundary.is_none() && self.context_store.has_session(session_id) {
            self.context_store.get_context_messages(session_id)
        } else {
            let snapshot = persistence
                .load_latest_turn_context_snapshot_before(
                    storage,
                    session_id,
                    boundary.unwrap_or(usize::MAX),
                )
                .await?;
            match snapshot {
                Some((index, mut messages)) => {
                    let metadata = persistence
                        .load_session_metadata(storage, session_id)
                        .await?;
                    if Self::listing_baseline_rebuild_turn_index_from_metadata(metadata.as_ref())
                        .is_some_and(|cutoff| index < cutoff)
                    {
                        messages = Self::strip_listing_diff_internal_reminders(messages).0;
                    }
                    let delta: Vec<_> = turns
                        .iter()
                        .filter(|turn| turn.turn_index > index)
                        .cloned()
                        .collect();
                    messages.extend(Self::build_messages_from_turns(&delta));
                    messages
                }
                None => Self::build_messages_from_turns(&turns),
            }
        };
        // Do not run cache restore/expiry cleanup: inspection must not write files.
        let prompt_cache = match self.prompt_cache_store.get_cache(session_id) {
            Some(cache) => cache,
            None => persistence
                .load_prompt_cache(storage, session_id)
                .await?
                .unwrap_or_default(),
        };
        let override_snapshot = match self
            .skill_agent_baseline_override_snapshot_store
            .get(session_id)
        {
            Some(snapshot) => Some(snapshot.value().clone()),
            None => {
                persistence
                    .load_skill_agent_baseline_override_snapshot(storage, session_id)
                    .await?
            }
        };
        let listing_baseline = match override_snapshot {
            Some(snapshot) => Some(snapshot),
            None => match self
                .turn_skill_agent_snapshot_store
                .get_snapshot(session_id, 0)
            {
                Some(snapshot) => Some(snapshot),
                None => {
                    persistence
                        .load_turn_skill_agent_snapshot(storage, session_id, 0)
                        .await?
                }
            },
        };
        Ok(ContextEstimateInputs {
            session,
            messages,
            prompt_cache,
            listing_baseline,
            turn_id,
            turn_metadata,
            turn_index,
            cache_ttl: self.config.prompt_cache_policy.cache_ttl,
        })
    }
}
