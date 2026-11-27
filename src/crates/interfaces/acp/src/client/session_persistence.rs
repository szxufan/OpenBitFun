use std::path::Path;
use std::sync::Arc;

use openbitfun_core::agentic::persistence::PersistenceManager;
use openbitfun_core::infrastructure::PathManager;
use openbitfun_core::service::session::{
    DialogTurnData, SessionMetadata, SessionTurnCatalog, SESSION_PROVIDER_ACP,
    SESSION_PROVIDER_METADATA_KEY,
};
use openbitfun_core::util::errors::{OpenBitFunError, OpenBitFunResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// Persistence reads these back to tell an externally projected Session from a
// Runtime-owned one, so both sides share one definition. It reaches this crate
// through the Session module it already reads metadata types from, because the
// ACP client owns no direct dependency on the core contracts.
pub(super) const CUSTOM_METADATA_PROVIDER_KEY: &str = SESSION_PROVIDER_METADATA_KEY;
pub(super) const CUSTOM_METADATA_PROVIDER_VALUE: &str = SESSION_PROVIDER_ACP;
pub(super) const CUSTOM_METADATA_CLIENT_ID_KEY: &str = "acpClientId";
pub(super) const CUSTOM_METADATA_REMOTE_SESSION_ID_KEY: &str = "acpRemoteSessionId";
const RESTORE_REQUIRED_KEY: &str = "acpRestoreRequired";
pub(super) const PENDING_TURN_KEY: &str = "acpPendingTurn";
const REPAIR_ATTEMPT_KEY: &str = "acpHistoryRepairAttempt";
pub(super) const UNSENT_FINISH_REASON: &str = "acp_not_sent";
pub(super) const CUSTOM_METADATA_RESUME_STRATEGY_KEY: &str = "acpResumeStrategy";
pub(super) const CUSTOM_METADATA_LAST_RESUME_ERROR_KEY: &str = "acpLastResumeError";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAcpFlowSessionRecordResponse {
    pub session_id: String,
    pub session_name: String,
    pub agent_type: String,
}

pub(super) struct AcpSessionPersistence {
    pub(super) manager: PersistenceManager,
}

impl AcpSessionPersistence {
    pub(super) fn new(path_manager: Arc<PathManager>) -> OpenBitFunResult<Self> {
        Ok(Self {
            manager: PersistenceManager::new(path_manager)?,
        })
    }

    pub(super) async fn create_flow_session_record(
        &self,
        session_storage_path: &Path,
        workspace_id: &str,
        workspace_path: &str,
        client_id: &str,
        session_name: Option<String>,
    ) -> OpenBitFunResult<CreateAcpFlowSessionRecordResponse> {
        let session_id = format!("acp_{}_{}", client_id, uuid::Uuid::new_v4());
        let agent_type = format!("acp:{}", client_id);
        let session_name = session_name
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("{} ACP", client_id));

        let mut metadata = SessionMetadata::new(
            session_id.clone(),
            session_name.clone(),
            agent_type.clone(),
            "primary".to_string(),
        );
        metadata.workspace_id = Some(workspace_id.to_string());
        metadata.workspace_path = Some(workspace_path.to_string());
        metadata.custom_metadata = Some(json!({
            "kind": "normal",
            CUSTOM_METADATA_PROVIDER_KEY: CUSTOM_METADATA_PROVIDER_VALUE,
            CUSTOM_METADATA_CLIENT_ID_KEY: client_id,
            CUSTOM_METADATA_REMOTE_SESSION_ID_KEY: null,
            CUSTOM_METADATA_RESUME_STRATEGY_KEY: null,
            CUSTOM_METADATA_LAST_RESUME_ERROR_KEY: null,
        }));

        if !self
            .manager
            .create_session_metadata_if_absent(session_storage_path, &metadata)
            .await?
        {
            return Err(OpenBitFunError::Validation(format!(
                "ACP flow session ID already exists: {}",
                session_id
            )));
        }

        Ok(CreateAcpFlowSessionRecordResponse {
            session_id,
            session_name,
            agent_type,
        })
    }

    pub(super) async fn delete_flow_session_record(
        &self,
        session_storage_path: &Path,
        openbitfun_session_id: &str,
    ) -> OpenBitFunResult<()> {
        self.manager
            .delete_session(session_storage_path, openbitfun_session_id)
            .await
    }

    pub(super) async fn prepare_dialog_turn(
        &self,
        path: &Path,
        session: &str,
        turn: &str,
        client: &str,
        input: &str,
        original_input: &str,
        user_metadata: Option<Value>,
    ) -> OpenBitFunResult<Option<(DialogTurnData, bool)>> {
        let record = self
            .manager
            .load_session_metadata(path, session)
            .await?
            .ok_or_else(|| {
                OpenBitFunError::NotFound(format!("ACP session record not found: {session}"))
            })?;
        let catalog = self
            .manager
            .load_session_turn_catalog(path, session, &[], record.turn_count)
            .await?;
        let first_owned = record
            .custom_metadata
            .as_ref()
            .and_then(|v| v.get("acpRuntimeFromTurn"))
            .and_then(Value::as_u64);
        let (index, existing) = self
            .free_turn_position(path, session, &catalog, turn)
            .await?;
        if existing {
            let previous = self
                .manager
                .load_dialog_turn(path, session, index)
                .await?
                .ok_or_else(|| {
                    OpenBitFunError::NotFound(format!("ACP turn record not found: {turn}"))
                })?;
            if previous.user_message.content != original_input {
                return Err(OpenBitFunError::validation(
                    "ACP turn id was already used with different input",
                ));
            }
            if first_owned.is_some_and(|first| index as u64 >= first)
                || previous.status != openbitfun_core::service::session::TurnStatus::InProgress
            {
                return Ok(None);
            }
        }
        let mut user_metadata = user_metadata;
        if input != original_input {
            let mut fields = match user_metadata.take() {
                Some(Value::Object(fields)) => fields,
                _ => serde_json::Map::new(),
            };
            fields.insert(super::transcript::PROMPT_TEXT_KEY.into(), input.into());
            user_metadata = Some(fields.into());
        }
        let data = super::transcript::start_turn(
            session,
            turn,
            index,
            client,
            original_input,
            user_metadata,
        );
        // A replay may append older turns. Accept the input durably now and
        // assign its final disk position after the agent restores the prefix.
        let pending = Self::has_history_gap(&record);
        if first_owned.is_none() || pending {
            self.update_binding(path, session, true, |metadata| {
                let fields = custom_fields(metadata);
                fields.entry("acpRuntimeFromTurn").or_insert(json!(index));
                if pending {
                    fields.insert(PENDING_TURN_KEY.into(), serde_json::to_value(&data)?);
                    fields.insert("acpHistoryGap".into(), true.into());
                }
                Ok(())
            })
            .await?;
        }
        if !pending {
            self.manager.save_dialog_turn(path, &data).await?;
        }
        Ok(Some((data, pending)))
    }

    /// A crash after the turn write and before clearing the receipt reuses the same index.
    pub(super) async fn place_pending_turn(
        &self,
        path: &Path,
        data: &mut DialogTurnData,
    ) -> OpenBitFunResult<()> {
        let session = &data.session_id;
        let record = self
            .manager
            .load_session_metadata(path, session)
            .await?
            .ok_or_else(|| {
                OpenBitFunError::NotFound(format!("ACP session record not found: {session}"))
            })?;
        let catalog = self
            .manager
            .load_session_turn_catalog(path, session, &[], record.turn_count)
            .await?;
        data.turn_index = self
            .free_turn_position(path, session, &catalog, &data.turn_id)
            .await?
            .0;
        if data.status != openbitfun_core::service::session::TurnStatus::InProgress {
            data.finish_reason = Some(UNSENT_FINISH_REASON.into());
        }
        self.commit_dialog_turn(path, data).await?;
        self.update_binding(path, session, true, |metadata| {
            custom_fields(metadata).remove(PENDING_TURN_KEY);
            Ok(())
        })
        .await
    }

    /// The catalog is truncated to the metadata turn count, which can lag behind
    /// the turn files after an interrupted write. Place a new turn after every
    /// stored file so it never takes a position that holds a different turn, and
    /// recognize a file that already stores this same turn.
    pub(super) async fn free_turn_position(
        &self,
        path: &Path,
        session: &str,
        catalog: &SessionTurnCatalog,
        turn_id: &str,
    ) -> OpenBitFunResult<(usize, bool)> {
        let (next, existing) = turn_position(catalog, turn_id);
        if existing {
            return Ok((next, true));
        }
        // A derived catalog entry without a turn id is a placeholder for a file it
        // has not read yet, and the retried turn can be any of them. Read those
        // files once and fold them back into the catalog, so the next lookup is
        // served by the complete catalog.
        let mut found = None;
        let mut read = Vec::new();
        for entry in catalog
            .entries
            .iter()
            .filter(|entry| entry.turn_id.is_none())
        {
            if let Some(stored) = self
                .manager
                .load_dialog_turn(path, session, entry.storage_turn_index)
                .await?
            {
                if stored.turn_id == turn_id {
                    found = Some(entry.storage_turn_index);
                }
                read.push(stored);
            }
        }
        if !read.is_empty() {
            if let Err(error) = self
                .manager
                .load_session_turn_catalog(path, session, &read, catalog.entries.len())
                .await
            {
                log::warn!(
                    "Failed to refresh the derived ACP turn catalog: session_id={session} error={error}"
                );
            }
        }
        if let Some(index) = found {
            return Ok((index, true));
        }
        let Some(stored_max) = self.manager.max_stored_turn_index(path, session).await? else {
            return Ok((next, false));
        };
        // Normally the catalog already covers every file and this range is empty.
        for index in next..=stored_max {
            if let Some(stored) = self.manager.load_dialog_turn(path, session, index).await? {
                if stored.turn_id == turn_id {
                    return Ok((index, true));
                }
            }
        }
        Ok((next.max(stored_max + 1), false))
    }

    /// First position after every stored turn file, at or after `from`.
    pub(super) async fn next_free_index(
        &self,
        path: &Path,
        session: &str,
        from: usize,
    ) -> OpenBitFunResult<usize> {
        Ok(self
            .manager
            .max_stored_turn_index(path, session)
            .await?
            .map_or(from, |stored_max| from.max(stored_max + 1)))
    }

    pub(super) async fn commit_dialog_turn(
        &self,
        path: &Path,
        data: &openbitfun_core::service::session::DialogTurnData,
    ) -> OpenBitFunResult<()> {
        if data.finish_reason.as_deref() == Some("acp_outcome_unknown") {
            self.update_binding(path, &data.session_id, true, |metadata| {
                custom_fields(metadata).insert("acpHistoryGap".into(), Value::Bool(true));
                Ok(())
            })
            .await?;
        }
        self.manager.save_dialog_turn(path, data).await
    }

    pub(super) fn has_history_gap(record: &SessionMetadata) -> bool {
        let custom = record.custom_metadata.as_ref().unwrap_or(&Value::Null);
        custom["acpHistoryGap"] == true
            || (record.turn_count == 0
                && (record.message_count > 0 || record.last_turn.is_some())
                && read_remote_binding(custom).0.is_some())
    }

    pub(super) async fn history_repaired(
        &self,
        path: &Path,
        session: &str,
    ) -> OpenBitFunResult<()> {
        self.update_binding(path, session, false, |metadata| {
            let fields = custom_fields(metadata);
            fields.remove("acpHistoryGap");
            fields.remove(REPAIR_ATTEMPT_KEY);
            Ok(())
        })
        .await
    }

    /// A replay that could not repair the gap would fail the same way again while
    /// the local history and the agent session stay the same. Remember the local
    /// state it was tried against so only a change retries the full replay.
    pub(super) fn repair_already_attempted(record: &SessionMetadata) -> bool {
        record
            .custom_metadata
            .as_ref()
            .and_then(|custom| custom.get(REPAIR_ATTEMPT_KEY))
            .and_then(Value::as_str)
            .is_some_and(|attempted| attempted == repair_fingerprint(record))
    }

    pub(super) async fn record_repair_attempt(
        &self,
        path: &Path,
        session: &str,
    ) -> OpenBitFunResult<()> {
        self.update_binding(path, session, false, |metadata| {
            let fingerprint = repair_fingerprint(metadata);
            custom_fields(metadata).insert(REPAIR_ATTEMPT_KEY.into(), fingerprint.into());
            Ok(())
        })
        .await
    }

    pub(super) async fn saved_options(
        &self,
        path: &Path,
        session: &str,
    ) -> OpenBitFunResult<super::AcpSessionOptions> {
        let metadata = self.manager.load_session_metadata(path, session).await?;
        let mut options = super::AcpSessionOptions::before_first_snapshot();
        if let Some(record) = metadata {
            if let Some(saved) = record
                .custom_metadata
                .as_ref()
                .and_then(|v| v.get("acpSessionOptions"))
            {
                options = serde_json::from_value(saved.clone())
                    .map_err(|error| OpenBitFunError::Deserialization(error.to_string()))?;
            }
            if options.current_model_id.is_none()
                && !record.model_name.is_empty()
                && record.model_name != "primary"
            {
                options.current_model_id = Some(record.model_name);
            }
        }
        Ok(options)
    }

    pub(super) async fn save_options(
        &self,
        path: &Path,
        session: &str,
        options: &super::AcpSessionOptions,
    ) -> OpenBitFunResult<()> {
        let snapshot = serde_json::to_value(options)
            .map_err(|err| OpenBitFunError::service(err.to_string()))?;
        self.update_binding(path, session, false, |metadata| {
            if let Some(model_id) = &options.current_model_id {
                metadata.model_name.clone_from(model_id);
            }
            custom_fields(metadata).insert("acpSessionOptions".into(), snapshot);
            Ok(())
        })
        .await
    }

    #[cfg(test)]
    pub(super) async fn remote_session_binding(
        &self,
        session_storage_path: &Path,
        openbitfun_session_id: &str,
    ) -> OpenBitFunResult<(Option<String>, bool)> {
        let Some(metadata) = self
            .manager
            .load_session_metadata(session_storage_path, openbitfun_session_id)
            .await?
        else {
            return Ok((None, false));
        };

        Ok(read_remote_binding(
            metadata.custom_metadata.as_ref().unwrap_or(&Value::Null),
        ))
    }

    pub(super) async fn require_restore(
        &self,
        session_storage_path: &Path,
        openbitfun_session_id: &str,
        remote_session_id: &str,
    ) -> OpenBitFunResult<()> {
        self.update_binding(
            session_storage_path,
            openbitfun_session_id,
            true,
            |metadata| {
                let custom = custom_fields(metadata);
                write_remote_binding(custom, remote_session_id, true);
                metadata.touch();
                Ok(())
            },
        )
        .await
    }

    pub(super) async fn update_remote_session_state(
        &self,
        session_storage_path: &Path,
        openbitfun_session_id: &str,
        remote_session_id: &str,
        resume_strategy: &str,
        last_resume_error: Option<String>,
        require_record: bool,
    ) -> OpenBitFunResult<()> {
        self.update_binding(
            session_storage_path,
            openbitfun_session_id,
            require_record,
            |metadata| {
                let custom = custom_fields(metadata);
                custom.insert(
                    CUSTOM_METADATA_PROVIDER_KEY.into(),
                    json!(CUSTOM_METADATA_PROVIDER_VALUE),
                );
                write_remote_binding(custom, remote_session_id, false);
                custom.insert(
                    CUSTOM_METADATA_RESUME_STRATEGY_KEY.into(),
                    json!(resume_strategy),
                );
                custom.insert(
                    CUSTOM_METADATA_LAST_RESUME_ERROR_KEY.into(),
                    last_resume_error.map(Value::String).unwrap_or(Value::Null),
                );
                metadata.touch();
                Ok(())
            },
        )
        .await
    }

    async fn update_binding(
        &self,
        session_storage_path: &Path,
        openbitfun_session_id: &str,
        require_record: bool,
        update: impl FnOnce(&mut SessionMetadata) -> OpenBitFunResult<()>,
    ) -> OpenBitFunResult<()> {
        let updated = self
            .manager
            .update_session_metadata_if_present(session_storage_path, openbitfun_session_id, update)
            .await?;
        if require_record && !updated {
            return Err(OpenBitFunError::NotFound(format!(
                "ACP session record not found: {openbitfun_session_id}"
            )));
        }
        Ok(())
    }
}

fn repair_fingerprint(record: &SessionMetadata) -> String {
    let custom = record.custom_metadata.as_ref().unwrap_or(&Value::Null);
    format!(
        "{}|{}|{}|{}",
        read_remote_binding(custom).0.unwrap_or_default(),
        record.turn_count,
        record
            .last_turn
            .as_ref()
            .map_or("", |turn| turn.turn_id.as_str()),
        custom
            .get(PENDING_TURN_KEY)
            .and_then(|pending| pending.get("turnId"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
}

fn custom_fields(metadata: &mut SessionMetadata) -> &mut serde_json::Map<String, Value> {
    let custom = metadata.custom_metadata.get_or_insert(Value::Null);
    if let Value::Object(fields) = custom {
        return fields;
    }
    let mut fields = serde_json::Map::new();
    if !custom.is_null() {
        fields.insert("acpLegacyCustomMetadata".into(), custom.take());
    }
    *custom = Value::Object(fields);
    custom.as_object_mut().expect("initialized an object")
}

fn turn_position(catalog: &SessionTurnCatalog, turn_id: &str) -> (usize, bool) {
    match catalog
        .entries
        .iter()
        .find(|entry| entry.turn_id.as_deref() == Some(turn_id))
    {
        Some(entry) => (entry.storage_turn_index, true),
        None => (
            catalog
                .entries
                .last()
                .map_or(0, |entry| entry.storage_turn_index + 1),
            false,
        ),
    }
}

pub(super) fn read_remote_binding(custom: &Value) -> (Option<String>, bool) {
    let id = custom
        .get(CUSTOM_METADATA_REMOTE_SESSION_ID_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    (id, custom[RESTORE_REQUIRED_KEY] == true)
}

fn write_remote_binding(
    fields: &mut serde_json::Map<String, Value>,
    remote_id: &str,
    require_restore: bool,
) {
    fields.insert(
        CUSTOM_METADATA_REMOTE_SESSION_ID_KEY.into(),
        json!(remote_id),
    );
    if require_restore {
        fields.insert(RESTORE_REQUIRED_KEY.into(), json!(true));
    } else {
        fields.remove(RESTORE_REQUIRED_KEY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repair_marker_only_holds_for_the_local_state_it_was_recorded_against() {
        let mut saved =
            SessionMetadata::new("s".into(), "chat".into(), "acp:x".into(), "primary".into());
        saved.custom_metadata =
            Some(json!({"acpRemoteSessionId": "remote", "acpHistoryGap": true}));
        assert!(!AcpSessionPersistence::repair_already_attempted(&saved));
        saved.custom_metadata.as_mut().unwrap()[REPAIR_ATTEMPT_KEY] = json!(3);
        assert!(!AcpSessionPersistence::repair_already_attempted(&saved));
        let marker = repair_fingerprint(&saved);
        saved.custom_metadata.as_mut().unwrap()[REPAIR_ATTEMPT_KEY] = json!(marker);
        assert!(AcpSessionPersistence::repair_already_attempted(&saved));
        saved.turn_count += 1;
        assert!(!AcpSessionPersistence::repair_already_attempted(&saved));
        saved.turn_count -= 1;
        saved.custom_metadata.as_mut().unwrap()["acpRemoteSessionId"] = json!("another");
        assert!(!AcpSessionPersistence::repair_already_attempted(&saved));
        saved.custom_metadata.as_mut().unwrap()["acpRemoteSessionId"] = json!("remote");
        saved.custom_metadata.as_mut().unwrap()[PENDING_TURN_KEY] = json!({"turnId": "pending"});
        assert!(!AcpSessionPersistence::repair_already_attempted(&saved));
    }
}
