use super::tests::test_client_connection;
use super::*;
use crate::client::execution::tests::Journal;
use crate::client::execution::AcpProjectionJournal;
use crate::client::prompt::tests::Harness;
use agent_client_protocol::schema::{
    AgentCapabilities, SessionCapabilities, SessionResumeCapabilities,
};
use openbitfun_core::agentic::persistence::PersistenceManager;
use openbitfun_core::service::session::{SessionStorageLayout, TurnStatus};
use openbitfun_core::service::{config::ConfigManagerSettings, session::SessionMetadata};

const CHAT: &str = "acp_chat";

struct TestSession {
    service: Arc<AcpClientService>,
    storage: PersistenceManager,
    journal: Arc<Journal>,
    root: tempfile::TempDir,
}

impl TestSession {
    async fn new() -> OpenBitFunResult<Self> {
        let root = tempfile::tempdir()?;
        let paths = Arc::new(PathManager::with_user_root_for_tests(
            root.path().join("user"),
        ));
        let config = ConfigService::with_settings(ConfigManagerSettings {
            path_manager: Some(paths.clone()),
            ..Default::default()
        })
        .await?;
        let storage = PersistenceManager::new(paths.clone())?;
        let service = AcpClientService::new(Arc::new(config), paths)?;
        let journal = Arc::new(Journal::default());
        service
            .configure_execution_journal(journal.clone())
            .map_err(OpenBitFunError::service)?;
        Ok(Self {
            service,
            storage,
            journal,
            root,
        })
    }

    fn layout(&self) -> SessionStorageLayout {
        SessionStorageLayout::new(
            self.storage
                .path_manager()
                .project_sessions_dir(self.root.path()),
        )
    }

    fn restart(&mut self) -> OpenBitFunResult<()> {
        self.service = AcpClientService::new(
            self.service.config_service.clone(),
            self.storage.path_manager().clone(),
        )?;
        self.service
            .configure_execution_journal(self.journal.clone())
            .map_err(OpenBitFunError::service)
    }

    async fn save_history(&self, custom: serde_json::Value) -> OpenBitFunResult<()> {
        let mut metadata = SessionMetadata::new(
            CHAT.into(),
            "chat".into(),
            "acp:opencode".into(),
            "primary".into(),
        );
        metadata.custom_metadata = Some(custom);
        self.storage
            .create_session_metadata_if_absent(self.root.path(), &metadata)
            .await?;
        Ok(())
    }

    async fn save_interrupted_prompts(&self, prompts: &[&str]) -> OpenBitFunResult<()> {
        self.save_history(json!({"acpRemoteSessionId":"original","acpHistoryGap":true}))
            .await?;
        for (index, &prompt) in prompts.iter().enumerate() {
            let mut turn = super::super::transcript::start_turn(
                CHAT,
                &format!("turn-{index}"),
                index,
                "opencode",
                prompt,
                None,
            );
            turn.user_message.id = format!("message-{index}");
            turn.status = TurnStatus::Error;
            turn.finish_reason = Some("acp_outcome_unknown".into());
            self.storage
                .save_dialog_turn(self.root.path(), &turn)
                .await?;
        }
        Ok(())
    }

    async fn connect(&self, capabilities: Option<AgentCapabilities>) -> Harness {
        let agent = Harness::new().await;
        let key = session_client_connection_id("opencode", CHAT);
        let client = test_client_connection(&key);
        *client.connection.write().await = Some(agent.active.connection());
        *client.agent_capabilities.write().await = Some(capabilities.unwrap_or_else(|| {
            AgentCapabilities::new()
                .load_session(true)
                .session_capabilities(
                    SessionCapabilities::new().resume(SessionResumeCapabilities::new()),
                )
        }));
        *client.status.write().await = AcpClientStatus::Running;
        self.service.clients.insert(key, client);
        agent
    }

    async fn needs_history(&self) -> OpenBitFunResult<bool> {
        Ok(self
            .storage
            .load_session_metadata(self.root.path(), CHAT)
            .await?
            .as_ref()
            .is_some_and(AcpSessionPersistence::has_history_gap))
    }

    async fn admit(&self, turn: &str, text: &str) -> OpenBitFunResult<Option<serde_json::Value>> {
        self.admit_with(turn, text, text).await
    }

    /// `sent` is the prompt text the agent receives; `shown` is the user's message.
    async fn admit_with(
        &self,
        turn: &str,
        sent: &str,
        shown: &str,
    ) -> OpenBitFunResult<Option<serde_json::Value>> {
        self.service
            .admit_dialog_turn(
                CHAT,
                turn,
                "opencode",
                sent,
                shown,
                None,
                Some(self.root.path()),
            )
            .await
            .map_err(OpenBitFunError::service)
    }

    async fn stream(
        &self,
        turn: &str,
        text: &str,
        timeout: Option<u64>,
        on_event: impl FnMut(AcpClientStreamEvent) -> OpenBitFunResult<()> + Send,
    ) -> OpenBitFunResult<()> {
        let path = self.root.path();
        self.service
            .prompt_agent_stream(
                "opencode",
                text.to_owned(),
                Some(path.display().to_string()),
                None,
                CHAT.into(),
                Some(turn),
                Some(path.into()),
                timeout,
                on_event,
            )
            .await
    }

    /// Open the session so the agent receives `method`, answering with `replay`.
    async fn open_with(
        &self,
        agent: &mut Harness,
        method: &str,
        replay: &[(&str, &str, Option<&str>)],
    ) -> OpenBitFunResult<()> {
        let path = self.root.path();
        let (options, ()) = tokio::join!(
            self.service.get_session_options(
                "opencode",
                Some(path.display().to_string()),
                None,
                Some(path.into()),
                CHAT.into(),
            ),
            async {
                let request = agent.request().await;
                assert_eq!(request["method"], method);
                for &(kind, text, id) in replay {
                    let mut update =
                        json!({"sessionUpdate":kind,"content":{"type":"text","text":text}});
                    if let Some(id) = id {
                        update["messageId"] = json!(id);
                    }
                    agent.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"original","update":update}})).await;
                }
                agent
                    .send(json!({"jsonrpc":"2.0","id":request["id"],"result":{}}))
                    .await;
            }
        );
        options.map(|_| ())
    }

    async fn prompt(&self, text: &str) -> OpenBitFunResult<String> {
        self.service
            .prompt_agent(
                "opencode",
                text.into(),
                Some(self.root.path().display().to_string()),
                None,
                CHAT.into(),
                Some(self.root.path().into()),
                Some(10),
            )
            .await
    }
}

#[tokio::test]
async fn queued_prompt_reloads_the_original_history_after_an_unconfirmed_stop(
) -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    session.save_history(json!("legacy")).await?;
    let mut old = session.connect(None).await;
    let mut queued = Box::pin(session.prompt("queued"));
    let (stopped, ()) = tokio::join!(session.prompt("edit once"), async {
        old.reply("session/new", json!({"result": {"sessionId": "original"}}))
            .await;
        assert_eq!(old.request().await["method"], "session/prompt");
        // Queue behind the same turn before requesting a stop that the agent never confirms.
        assert!(futures::poll!(queued.as_mut()).is_pending());
        assert!(session
            .service
            .cancel_openbitfun_session(CHAT)
            .await
            .unwrap());
        assert_eq!(old.request().await["method"], "session/cancel");
    });
    assert!(matches!(stopped, Err(OpenBitFunError::OutcomeUnknown(_))));
    let saved = session
        .storage
        .load_session_metadata(session.root.path(), CHAT)
        .await?
        .unwrap();
    let fields = saved.custom_metadata.unwrap();
    let binding = || {
        session
            .service
            .session_persistence
            .remote_session_binding(session.root.path(), CHAT)
    };
    assert_eq!(binding().await?, (Some("original".into()), true));
    assert_eq!(fields["acpLegacyCustomMetadata"], "legacy");

    let mut agent = session.connect(None).await;
    let offline = json!({"error": {"code": -32603, "message": "temporarily offline"}});
    let (failed, load) = tokio::join!(queued, agent.reply("session/load", offline));
    assert_eq!(load["sessionId"], "original");
    assert!(
        matches!(failed, Err(OpenBitFunError::Session(reason)) if reason.contains("temporarily offline"))
    );

    let (retried, ()) = tokio::join!(session.prompt("fresh"), async {
        let load = agent.reply("session/load", json!({"result": {}})).await;
        assert_eq!(load["sessionId"], "original");
        let request = agent.finish_with_text("back online", "end_turn").await;
        assert_eq!(request["sessionId"], "original");
        assert_eq!(request["prompt"][0]["text"], "fresh");
    });
    assert_eq!(retried?, "back online");
    assert_eq!(binding().await?, (Some("original".into()), false));
    Ok(())
}

#[tokio::test]
async fn a_failed_restore_keeps_the_original_session_for_retry() -> OpenBitFunResult<()> {
    for stop_unconfirmed in [false, true] {
        let session = TestSession::new().await?;
        session
            .save_history(json!({
                "acpRemoteSessionId": "original",
                "acpRestoreRequired": stop_unconfirmed,
            }))
            .await?;
        let mut agent = session.connect(None).await;
        let method = if stop_unconfirmed {
            "session/load"
        } else {
            "session/resume"
        };
        let (failed, request) = tokio::join!(
            session.prompt("first"),
            agent.reply(
                method,
                json!({"error": {"code": -32603, "message": "offline"}}),
            )
        );
        assert_eq!(request["sessionId"], "original");
        assert!(failed.unwrap_err().to_string().contains("offline"));
        let binding = || {
            session
                .service
                .session_persistence
                .remote_session_binding(session.root.path(), CHAT)
        };
        assert_eq!(
            binding().await?,
            (Some("original".into()), stop_unconfirmed)
        );

        let (retried, ()) = tokio::join!(session.prompt("retry"), async {
            if !stop_unconfirmed {
                let resume = agent
                    .reply(
                        "session/resume",
                        json!({"error":{"code":-32601,"message":"method unavailable"}}),
                    )
                    .await;
                assert_eq!(resume["sessionId"], "original");
            }
            let load = agent.reply("session/load", json!({"result": {}})).await;
            assert_eq!(load["sessionId"], "original");
            let prompt = agent.finish_with_text("reply", "end_turn").await;
            assert_eq!(prompt["sessionId"], "original");
            assert_eq!(prompt["prompt"][0]["text"], "retry");
        });
        assert_eq!(retried?, "reply");
        assert_eq!(binding().await?, (Some("original".into()), false));
    }
    Ok(())
}

#[tokio::test]
async fn an_agent_without_restore_keeps_the_binding_and_the_unsent_input() -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    let path = session.root.path();
    session
        .save_history(json!({"acpRemoteSessionId":"original"}))
        .await?;
    session.admit("attempt", "continue").await?;
    let mut agent = session.connect(Some(AgentCapabilities::new())).await;
    let error = tokio::select! {
        result = session.stream("attempt", "continue", Some(5), |_| Ok(())) => result.unwrap_err(),
        request = agent.request() => panic!("unexpected request on a saved session: {request}"),
    };
    let mut terminal = json!({"sessionId":CHAT,"turnId":"attempt","error":error.to_string()});
    session
        .service
        .finish_dialog_turn(
            CHAT,
            Some(path),
            "agentic://dialog-turn-failed",
            &mut terminal,
        )
        .await?;
    let saved = session
        .storage
        .load_dialog_turn(path, CHAT, 0)
        .await?
        .unwrap();
    assert_eq!(saved.status, TurnStatus::Error);
    assert_eq!(saved.finish_reason.as_deref(), Some("acp_not_sent"));
    let reason = saved.error.unwrap();
    assert!(reason.contains("does not support resuming or loading"));
    assert!(reason.contains("Start a new chat"));
    assert_eq!(
        session
            .service
            .session_persistence
            .remote_session_binding(path, CHAT)
            .await?,
        (Some("original".into()), false)
    );
    Ok(())
}

#[tokio::test]
async fn missing_record_allows_sending_but_blocks_required_recovery() -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    let mut agent = session.connect(None).await;
    let stream = session.service.prompt_agent_stream(
        "opencode",
        "edit once".into(),
        Some(session.root.path().display().to_string()),
        None,
        CHAT.into(),
        None,
        Some(session.root.path().into()),
        Some(10),
        |_| Ok(()),
    );
    let (stopped, ()) = tokio::join!(stream, async {
        agent
            .reply("session/new", json!({"result": {"sessionId": "original"}}))
            .await;
        let request = agent.request().await;
        assert_eq!(request["method"], "session/prompt");
        assert_eq!(request["params"]["prompt"][0]["text"], "edit once");
        assert!(session
            .service
            .cancel_openbitfun_session(CHAT)
            .await
            .unwrap());
    });
    assert!(
        matches!(stopped, Err(OpenBitFunError::OutcomeUnknown(reason)) if reason.contains("could not be saved"))
    );

    let mut replacement = session.connect(None).await;
    let (failed, load) = tokio::join!(
        session.prompt("retry"),
        replacement.reply("session/load", json!({"result": {}}))
    );
    assert_eq!(load["sessionId"], "original");
    assert!(matches!(failed, Err(OpenBitFunError::NotFound(_))));
    assert!(!session.service.cancel_openbitfun_session(CHAT).await?);
    Ok(())
}

#[tokio::test]
async fn cancelled_tool_prompts_never_return_successful_text() -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    session
        .save_history(json!({"acpRemoteSessionId": "original"}))
        .await?;
    let mut agent = session.connect(None).await;
    *session
        .service
        .clients
        .get(&session_client_connection_id("opencode", CHAT))
        .unwrap()
        .agent_capabilities
        .write()
        .await = Some(AgentCapabilities::new().load_session(true));
    let (before_submit, ()) = tokio::join!(session.prompt("never send"), async {
        agent.reply("session/load", json!({"result": {}})).await;
        // Stop through the public entry point while the manager is draining load replay.
        tokio::time::timeout(Duration::from_secs(2), async {
            while !session
                .service
                .cancel_openbitfun_session(CHAT)
                .await
                .unwrap()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("loaded session must become cancellable");
        assert_eq!(agent.request().await["method"], "session/cancel");
    });
    assert!(matches!(before_submit, Err(OpenBitFunError::Cancelled(_))));

    let (after_submit, ()) = tokio::join!(session.prompt("next"), async {
        let request = agent.finish_with_text("partial", "cancelled").await;
        assert_eq!(request["sessionId"], "original");
        assert_eq!(request["prompt"][0]["text"], "next");
    });
    assert!(matches!(after_submit, Err(OpenBitFunError::Cancelled(_))));
    Ok(())
}

#[tokio::test]
async fn background_turn_survives_view_detach_and_host_restart() -> OpenBitFunResult<()> {
    let mut session = TestSession::new().await?;
    session.save_history(json!({"provider":"acp"})).await?;
    let path = session.root.path().to_owned();
    let metadata = json!({"original_text":"run once"});
    let started = session
        .service
        .admit_dialog_turn(
            CHAT,
            "turn-a",
            "opencode",
            "run once",
            "run once",
            Some(metadata.clone()),
            Some(&path),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        started["userMessageMetadata"]["acpRuntimeOwnsHistory"],
        true
    );
    for (name, extra) in [
        (
            "agentic://model-round-started",
            json!({"roundId":"round-a", "roundIndex":0}),
        ),
        ("agentic://text-chunk", json!({"text":"before the tool"})),
        (
            "agentic://tool-event",
            json!({"toolEvent":{"event_type":"Started","tool_id":"tool-a","tool_name":"read","params":{"path":"example"}}}),
        ),
        (
            "agentic://tool-event",
            json!({"toolEvent":{"event_type":"Completed","tool_id":"tool-a","tool_name":"read","result":"done","duration_ms":4}}),
        ),
        ("agentic://text-chunk", json!({"text":"after the tool"})),
    ] {
        let mut payload = extra;
        payload["sessionId"] = CHAT.into();
        payload["turnId"] = "turn-a".into();
        payload["roundId"] = "round-a".into();
        session
            .service
            .record_dialog_event(name, &mut payload)
            .unwrap();
    }
    let stale = session
        .storage
        .load_dialog_turn(&path, CHAT, 0)
        .await?
        .unwrap();
    assert!(stale.model_rounds.is_empty());
    let session_dir = session
        .storage
        .path_manager()
        .project_sessions_dir(&path)
        .join(CHAT);
    let backup = path.join("saved-session");
    std::fs::rename(&session_dir, &backup)?;
    std::fs::write(&session_dir, "storage unavailable")?;
    let mut terminal = json!({"sessionId":CHAT, "turnId":"turn-a"});
    session
        .service
        .finish_dialog_turn(
            CHAT,
            Some(&path),
            "agentic://dialog-turn-completed",
            &mut terminal,
        )
        .await?;
    assert!(terminal["acpHistorySaveError"].is_string());
    assert!(!session.service.execution_snapshot(CHAT).unwrap().running);
    assert!(matches!(
        session.journal.pending_checkpoint(CHAT).last(),
        Some(openbitfun_events::AgenticEvent::DialogTurnCompleted { .. })
    ));
    std::fs::remove_file(&session_dir)?;
    std::fs::rename(&backup, &session_dir)?;
    session
        .service
        .recover_stored_dialog_turn(&path, CHAT)
        .await?;
    assert!(session.journal.pending_checkpoint(CHAT).is_empty());
    session
        .storage
        .save_external_view_checkpoint(&path, &stale)
        .await?;
    session.restart()?;
    session
        .service
        .recover_stored_dialog_turn(&path, CHAT)
        .await?;
    let saved = session
        .storage
        .load_dialog_turn(&path, CHAT, 0)
        .await?
        .unwrap();
    assert_eq!(saved.status, TurnStatus::Completed);
    assert_eq!(saved.user_message.metadata, Some(metadata));
    let round = &saved.model_rounds[0];
    assert_eq!(round.text_items[0].content, "before the tool");
    assert_eq!(round.tool_items[0].tool_call.input["path"], "example");
    assert_eq!(
        round.tool_items[0].tool_result.as_ref().unwrap().result,
        "done"
    );
    assert_eq!(round.text_items[1].content, "after the tool");
    assert!(session.admit("turn-a", "run once").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn stopping_during_session_restore_settles_before_the_agent_replies() -> OpenBitFunResult<()>
{
    for (method, history) in [
        ("session/resume", json!({"acpRemoteSessionId":"original"})),
        (
            "session/load",
            json!({"acpRemoteSessionId":"original","acpRestoreRequired":true}),
        ),
    ] {
        let session = TestSession::new().await?;
        let path = session.root.path();
        session.save_history(history).await?;
        session.admit("turn", "hello").await?;
        let mut agent = session.connect(None).await;
        let mut cancelled = false;
        let mut worker = Box::pin(session.stream("turn", "hello", None, |event| {
            cancelled |= matches!(event, AcpClientStreamEvent::Cancelled);
            Ok(())
        }));
        let restore = tokio::select! {
            request = agent.request() => request,
            result = &mut worker => panic!("turn settled before {method}: {result:?}"),
        };
        assert_eq!(restore["method"], method);
        session
            .service
            .cancel_agent_session(
                "opencode",
                Some(path.display().to_string()),
                CHAT.into(),
                Some("turn"),
            )
            .await?;
        tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .expect("turn should settle while the restore response is withheld")?;
        assert!(cancelled);
        agent
            .send(json!({"jsonrpc":"2.0","id":restore["id"],"result":{}}))
            .await;

        let mut terminal = json!({"sessionId":CHAT,"turnId":"turn"});
        session
            .service
            .finish_dialog_turn(
                CHAT,
                Some(path),
                "agentic://dialog-turn-cancelled",
                &mut terminal,
            )
            .await?;
        let saved = session
            .storage
            .load_dialog_turn(path, CHAT, 0)
            .await?
            .unwrap();
        assert_eq!(saved.finish_reason.as_deref(), Some("acp_not_sent"));
        let (reply, next) = tokio::join!(
            session.prompt("next"),
            agent.finish_with_text("ok", "end_turn")
        );
        assert_eq!(next["prompt"][0]["text"], "next");
        assert_eq!(reply?, "ok");
    }
    Ok(())
}

#[tokio::test]
async fn opening_a_legacy_session_and_sending_share_one_restore() -> OpenBitFunResult<()> {
    let mut session = TestSession::new().await?;
    let path = session.root.path().to_owned();
    session
        .save_history(json!({"provider": "acp", "acpRemoteSessionId": "original"}))
        .await?;
    let missing = session
        .service
        .observe_session_options("opencode", CHAT, path.to_str(), Some(&path))
        .await?;
    assert!(missing.needs_initial_options);
    assert!(missing.current_model_id.is_none());
    assert!(session.service.clients.is_empty());

    let mut agent = session.connect(None).await;
    let (options, reply) = tokio::join!(
        session.service.get_session_options(
            "opencode",
            Some(path.display().to_string()),
            None,
            Some(path.clone()),
            CHAT.into(),
        ),
        async {
            let restore = agent.request().await;
            assert_eq!(restore["method"], "session/resume");
            assert_eq!(restore["params"]["sessionId"], "original");
            let (reply, ()) = tokio::join!(session.prompt("hello"), async {
                agent.send(json!({"jsonrpc":"2.0", "id":restore["id"], "result":{
                    "models":{"currentModelId":"current","availableModels":[{"modelId":"current","name":"Current"}]}
                }})).await;
                let prompt = agent.finish_with_text("answer", "end_turn").await;
                assert_eq!(prompt["sessionId"], "original");
                assert_eq!(prompt["prompt"][0]["text"], "hello");
            });
            reply
        },
    );
    assert_eq!(options?.current_model_id.as_deref(), Some("current"));
    assert_eq!(reply?, "answer");

    session.restart()?;
    let cached = session
        .service
        .observe_session_options("opencode", CHAT, path.to_str(), Some(&path))
        .await?;
    assert!(cached.agent_disconnected);
    assert!(!cached.needs_initial_options);
    assert_eq!(cached.current_model_id.as_deref(), Some("current"));
    assert!(session.service.clients.is_empty());
    Ok(())
}

#[tokio::test]
async fn interrupted_output_is_reconciled_before_sending_the_next_turn() -> OpenBitFunResult<()> {
    let mut session = TestSession::new().await?;
    session
        .save_history(json!({"acpRemoteSessionId":"original","provider":"acp"}))
        .await?;
    let path = session.root.path().to_owned();
    session.admit("interrupted", "first").await?;
    for (name, mut payload) in [
        ("agentic://model-round-started", json!({"roundIndex":0})),
        ("agentic://text-chunk", json!({"text":"partial"})),
    ] {
        payload["sessionId"] = CHAT.into();
        payload["turnId"] = "interrupted".into();
        payload["roundId"] = "r".into();
        session
            .service
            .record_dialog_event(name, &mut payload)
            .unwrap();
    }
    session.restart()?;
    session
        .service
        .recover_stored_dialog_turn(&path, CHAT)
        .await?;
    assert!(session.service.clients.is_empty());
    let saved = session
        .storage
        .load_dialog_turn(&path, CHAT, 0)
        .await?
        .unwrap();
    assert_eq!(saved.model_rounds[0].text_items[0].content, "partial");
    assert_eq!(saved.finish_reason.as_deref(), Some("acp_outcome_unknown"));
    assert_eq!(saved.model_rounds[0].status, "error");
    assert!(session.needs_history().await?);

    let admit = || session.admit("next-turn", "next");
    let receipt = admit().await.unwrap().unwrap();
    assert!(receipt["turnIndex"].is_null());
    assert!(session.service.clients.is_empty());
    let mut agent = session.connect(None).await;
    let id = saved.user_message.id;
    let (result, ()) = tokio::join!(
        session.stream("next-turn", "next", None, |_| Ok(()),),
        async {
            let request = agent.request().await;
            assert_eq!(request["method"], "session/load");
            assert_eq!(request["params"]["sessionId"], "original");
            tokio::time::pause();
            tokio::time::sleep(Duration::from_secs(45)).await;
            assert!(admit().await.unwrap().is_none());
            for (kind, text, id) in [
                ("user_message_chunk", "first", Some(&id)),
                ("agent_message_chunk", "partial recovered", None),
                ("user_message_chunk", "first", None),
                ("agent_message_chunk", "restored output", None),
            ] {
                let mut update =
                    json!({"sessionUpdate":kind, "content":{"type":"text","text":text}});
                if let Some(id) = id {
                    update["messageId"] = json!(id);
                }
                agent.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"original","update":update}})).await;
                if kind == "user_message_chunk" {
                    // A progressing history replay can legitimately span several startup timeouts.
                    tokio::time::sleep(Duration::from_secs(45)).await;
                }
            }
            tokio::time::resume();
            agent
                .send(json!({"jsonrpc":"2.0","id":request["id"],"result":{}}))
                .await;
            let sent = agent.finish_with_text("done", "end_turn").await;
            assert_eq!(sent["sessionId"], "original");
            assert_eq!(sent["prompt"][0]["text"], "next");
        }
    );
    result?;
    let mut done = json!({"sessionId": CHAT, "turnId": "next-turn"});
    session
        .service
        .finish_dialog_turn(
            CHAT,
            Some(&path),
            "agentic://dialog-turn-completed",
            &mut done,
        )
        .await?;
    let repaired = session
        .storage
        .load_visible_session_turns(&path, CHAT)
        .await?;
    assert_eq!(repaired.len(), 3);
    assert_eq!(
        repaired[1].model_rounds[0].text_items[0].content,
        "restored output"
    );
    assert_eq!(
        (repaired[2].turn_index, repaired[2].turn_id.as_str()),
        (2, "next-turn")
    );
    assert_eq!(
        repaired[0].model_rounds[0].text_items[0].content,
        "partial recovered"
    );
    assert_eq!(repaired[0].turn_id, "interrupted");
    assert_eq!(
        repaired[0].finish_reason.as_deref(),
        Some("acp_outcome_unknown")
    );
    assert!(!session.needs_history().await?);
    Ok(())
}

#[tokio::test]
async fn a_legacy_interrupted_turn_keeps_its_richer_checkpoint_after_partial_replay(
) -> OpenBitFunResult<()> {
    expect_legacy_checkpoint_preserved(&["already"]).await
}

#[tokio::test]
async fn repeated_prompts_without_message_ids_leave_ambiguous_history_intact(
) -> OpenBitFunResult<()> {
    expect_legacy_checkpoint_preserved(&["already saved first reply", "already saved second reply"])
        .await
}

#[tokio::test]
async fn adjacent_user_chunks_need_a_message_id_to_recover_one_prompt() -> OpenBitFunResult<()> {
    for id in [None, Some("c52081b3-45ff-44ab-9c12-48ed2cd97fd4")] {
        let session = TestSession::new().await?;
        let path = session.root.path();
        session
            .save_history(json!({"acpRemoteSessionId":"original","acpHistoryGap":true}))
            .await?;
        let mut known =
            super::super::transcript::start_turn(CHAT, "known", 0, "opencode", "earlier", None);
        known.status = TurnStatus::Error;
        session.storage.save_dialog_turn(path, &known).await?;
        let mut agent = session.connect(None).await;
        let (options, ()) = tokio::join!(
            session.service.get_session_options(
                "opencode",
                Some(path.display().to_string()),
                None,
                Some(path.into()),
                CHAT.into()
            ),
            async {
                let load = agent.request().await;
                assert_eq!(load["method"], "session/load");
                for (kind, text, id) in [
                    (
                        "user_message_chunk",
                        "earlier",
                        Some(known.user_message.id.as_str()),
                    ),
                    ("agent_message_chunk", "recovered answer", None),
                    ("user_message_chunk", "hello", id),
                    ("user_message_chunk", " again", id),
                ] {
                    let mut update =
                        json!({"sessionUpdate":kind,"content":{"type":"text","text":text}});
                    if let Some(id) = id {
                        update["messageId"] = json!(id);
                    }
                    agent.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"original","update":update}})).await;
                }
                agent
                    .send(json!({"jsonrpc":"2.0","id":load["id"],"result":{}}))
                    .await;
            }
        );
        options?;
        let saved = session
            .storage
            .load_visible_session_turns(path, CHAT)
            .await?;
        assert_eq!(saved[0].turn_id, "known");
        assert_eq!(
            saved[0].model_rounds[0].text_items[0].content,
            "recovered answer"
        );
        if let Some(id) = id {
            assert_eq!(saved.len(), 2);
            assert_eq!(saved[1].user_message.id, id);
            assert_eq!(saved[1].user_message.content, "hello again");
            assert!(!session.needs_history().await?);
        } else {
            assert_eq!(saved.len(), 1);
            assert!(session.needs_history().await?);
        }
    }
    Ok(())
}

async fn expect_legacy_checkpoint_preserved(replies: &[&str]) -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    let path = session.root.path();
    session
        .save_history(json!({"provider":"acp","acpRemoteSessionId":"original"}))
        .await?;
    let mut old =
        super::super::transcript::start_turn(CHAT, "old-turn", 0, "opencode", "continue", None);
    super::super::transcript::start_round(&mut old, "old-round", 0, 42);
    super::super::transcript::append_text(&mut old.model_rounds[0], "already saved", false, 42);
    session.storage.save_dialog_turn(path, &old).await?;
    session
        .service
        .recover_stored_dialog_turn(path, CHAT)
        .await?;
    let saved = session
        .storage
        .load_dialog_turn(path, CHAT, 0)
        .await?
        .unwrap();
    assert_eq!(saved.model_rounds[0].id, "old-round");
    assert_eq!(saved.finish_reason.as_deref(), Some("acp_outcome_unknown"));
    let mut agent = session.connect(None).await;
    let (recovered, ()) = tokio::join!(
        session.service.get_session_options(
            "opencode",
            Some(path.display().to_string()),
            None,
            Some(path.into()),
            CHAT.into()
        ),
        async {
            let load = agent.request().await;
            assert_eq!(load["method"], "session/load");
            for &reply in replies {
                for (kind, text) in [
                    ("user_message_chunk", "continue"),
                    ("agent_message_chunk", reply),
                ] {
                    agent.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"original","update":{"sessionUpdate":kind,"content":{"type":"text","text":text}}}})).await;
                }
            }
            agent
                .send(json!({"jsonrpc":"2.0","id":load["id"],"result":{}}))
                .await;
        }
    );
    recovered?;
    let turns = session
        .storage
        .load_visible_session_turns(path, CHAT)
        .await?;
    assert_eq!(turns.len(), 1);
    assert_eq!(
        turns[0].model_rounds[0].text_items[0].content,
        "already saved"
    );
    assert!(session.needs_history().await?);
    Ok(())
}

#[tokio::test]
async fn a_prompt_deadline_preserves_its_binding_and_unknown_outcome() -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    let path = session.root.path();
    session.save_history(json!({})).await?;
    let mut agent = session.connect(None).await;
    let run = session.service.prompt_agent(
        "opencode",
        "edit once".into(),
        Some(path.display().to_string()),
        None,
        CHAT.into(),
        Some(path.into()),
        Some(1),
    );
    let (result, ()) = tokio::join!(run, async {
        agent
            .reply("session/new", json!({"result":{"sessionId":"original"}}))
            .await;
        assert_eq!(agent.request().await["method"], "session/prompt");
    });
    assert!(matches!(result, Err(OpenBitFunError::OutcomeUnknown(_))));
    assert_eq!(
        session
            .service
            .session_persistence
            .remote_session_binding(path, CHAT)
            .await?,
        (Some("original".into()), true)
    );
    Ok(())
}

#[tokio::test]
async fn confirmed_load_recovers_history_offline_without_overwriting_an_active_turn(
) -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    let path = session.root.path();
    session
        .save_history(
            json!({"provider":"acp","acpRemoteSessionId":"original","acpHistoryGap":true}),
        )
        .await?;
    let local = super::super::transcript::start_turn(CHAT, "local", 0, "opencode", "first", None);
    session.storage.save_dialog_turn(path, &local).await?;
    let mut agent = session.connect(None).await;
    let client = session
        .service
        .clients
        .get(&session_client_connection_id("opencode", CHAT))
        .unwrap()
        .clone();
    let (options, ()) = tokio::join!(
        session.service.get_session_options(
            "opencode",
            Some(path.display().to_string()),
            None,
            Some(path.into()),
            CHAT.into()
        ),
        async {
            let request = agent.request().await;
            assert_eq!(request["method"], "session/load");
            for (kind, text) in [
                ("user_message_chunk", "first"),
                ("agent_message_chunk", "partial"),
                ("user_message_chunk", "second"),
                ("agent_message_chunk", "saved answer"),
            ] {
                agent.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"original",
                    "update":{"sessionUpdate":kind,"content":{"type":"text","text":text}}}})).await;
            }
            agent.send(json!({"jsonrpc":"2.0","id":request["id"],"result":{
                "models":{"currentModelId":"chosen","availableModels":[{"modelId":"chosen","name":"Chosen"}]}}})).await;
            // The connection monitor can retire the transport before the confirmed response is consumed.
            *client.connection.write().await = None;
        }
    );
    let options = options?;
    assert!(options.agent_disconnected);
    assert_eq!(options.current_model_id.as_deref(), Some("chosen"));
    let turns = session
        .storage
        .load_visible_session_turns(path, CHAT)
        .await?;
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].status, TurnStatus::InProgress);
    assert!(turns[0].model_rounds.is_empty());
    assert_eq!(
        turns[1].model_rounds[0].text_items[0].content,
        "saved answer"
    );
    assert!(session.needs_history().await?);
    Ok(())
}

#[tokio::test]
async fn a_stalled_load_reconnects_to_the_original_session_without_rewriting_complete_history(
) -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    let path = session.root.path();
    session
        .save_history(json!({"provider":"acp","acpRemoteSessionId":"original"}))
        .await?;
    let key = session_client_connection_id("opencode", CHAT);
    let get_options = || {
        session.service.get_session_options(
            "opencode",
            Some(path.display().to_string()),
            None,
            Some(path.into()),
            CHAT.into(),
        )
    };
    for deadline in [CLIENT_STARTUP_TIMEOUT, SESSION_LOAD_TIMEOUT] {
        let mut agent = session.connect(None).await;
        let old = session.service.clients.get(&key).unwrap().clone();
        *old.agent_capabilities.write().await = Some(AgentCapabilities::new().load_session(true));
        let (result, ()) = tokio::join!(get_options(), async {
            let request = agent.request().await;
            assert_eq!(request["method"], "session/load");
            assert_eq!(request["params"]["sessionId"], "original");
            tokio::time::pause();
            if deadline == SESSION_LOAD_TIMEOUT {
                let interval = CLIENT_STARTUP_TIMEOUT / 2;
                for _ in 1..deadline.as_secs() / interval.as_secs() {
                    tokio::time::sleep(interval).await;
                    agent.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"original",
                        "update":{"sessionUpdate":"available_commands_update","availableCommands":[]}}})).await;
                }
                tokio::time::sleep(interval + Duration::from_secs(1)).await;
            } else {
                tokio::time::advance(deadline + Duration::from_secs(1)).await;
            }
            tokio::time::resume();
        });
        let error = result.unwrap_err();
        assert!(error.to_string().contains(STARTUP_TIMEOUT_ERROR_PREFIX));
        assert!(error
            .to_string()
            .contains(&format!("{}s", deadline.as_secs())));
        assert_eq!(*old.status.read().await, AcpClientStatus::Stopped);
        assert!(!session.service.clients.contains_key(&key));
    }
    let mut replacement = session.connect(None).await;
    let client = session.service.clients.get(&key).unwrap().clone();
    *client.agent_capabilities.write().await = Some(AgentCapabilities::new().load_session(true));
    let (result, ()) = tokio::join!(get_options(), async {
        let request = replacement.request().await;
        assert_eq!(request["method"], "session/load");
        assert_eq!(request["params"]["sessionId"], "original");
        for (kind, text) in [
            ("user_message_chunk", "older input"),
            ("agent_message_chunk", "older output"),
        ] {
            replacement.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"original","update":{"sessionUpdate":kind,"content":{"type":"text","text":text}}}})).await;
        }
        replacement.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"original","update":{"sessionUpdate":"tool_call","toolCallId":"old-without-title"}}})).await;
        replacement.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"original","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"review","description":"Review"}]}}})).await;
        replacement
            .send(json!({"jsonrpc":"2.0","id":request["id"],"result":{}}))
            .await;
    });
    assert_eq!(result?.available_commands.unwrap()[0].name, "review");
    assert!(session
        .storage
        .load_visible_session_turns(path, CHAT)
        .await?
        .is_empty());
    Ok(())
}

#[tokio::test]
async fn restarting_during_history_repair_keeps_the_accepted_unsent_input() -> OpenBitFunResult<()>
{
    let mut session = TestSession::new().await?;
    session
        .save_history(
            json!({"provider":"acp","acpHistoryGap":true,"acpRemoteSessionId":"original"}),
        )
        .await?;
    let path = session.root.path().to_owned();
    let received = session.admit("accepted", "new").await?;
    assert!(received.is_some());
    assert!(session.service.clients.is_empty());
    session.restart()?;
    session
        .service
        .recover_stored_dialog_turn(&path, CHAT)
        .await?;
    assert!(session.admit("accepted", "new").await?.is_none());
    let turns = session
        .storage
        .load_visible_session_turns(&path, CHAT)
        .await?;
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].user_message.content, "new");
    assert_eq!(turns[0].status, TurnStatus::Error);
    assert_eq!(turns[0].finish_reason.as_deref(), Some("acp_not_sent"));
    assert!(turns[0]
        .error
        .as_deref()
        .unwrap()
        .contains("before the ACP prompt was sent"));
    assert!(session.service.clients.is_empty());

    assert!(session.admit("second", "again").await?.is_some());
    let mut agent = session.connect(None).await;
    let mut cancelled = false;
    let (result, ()) = tokio::join!(
        session.stream("second", "again", Some(5), |event| {
            cancelled |= matches!(event, AcpClientStreamEvent::Cancelled);
            Ok(())
        },),
        async {
            let request = agent.request().await;
            assert_eq!(request["method"], "session/load");
            session
                .service
                .cancel_agent_session(
                    "opencode",
                    Some(path.display().to_string()),
                    CHAT.into(),
                    Some("second"),
                )
                .await
                .unwrap();
            // Same text as the unsent receipt; it belongs to an earlier remote turn.
            agent.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"original","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"new"}}}})).await;
            agent.send(json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"original","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"earlier remote reply"}}}})).await;
            agent
                .send(json!({"jsonrpc":"2.0","id":request["id"],"result":{}}))
                .await;
        }
    );
    result?;
    assert!(cancelled);
    let mut terminal = json!({"sessionId": CHAT, "turnId": "second"});
    session
        .service
        .finish_dialog_turn(
            CHAT,
            Some(&path),
            "agentic://dialog-turn-cancelled",
            &mut terminal,
        )
        .await?;
    let turns = session
        .storage
        .load_visible_session_turns(&path, CHAT)
        .await?;
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].turn_id, "accepted");
    assert_eq!(turns[0].finish_reason.as_deref(), Some("acp_not_sent"));
    assert_eq!(turns[1].turn_id, "second");
    assert_eq!(turns[1].status, TurnStatus::Cancelled);
    assert_eq!(turns[1].finish_reason.as_deref(), Some("acp_not_sent"));
    assert!(session.needs_history().await?);
    Ok(())
}

#[tokio::test]
async fn a_lagging_turn_count_never_overwrites_a_saved_turn() -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    let path = session.root.path();
    session.save_history(json!({"provider": "acp"})).await?;
    let mut saved =
        super::super::transcript::start_turn(CHAT, "saved", 2, "opencode", "earlier", None);
    saved.status = TurnStatus::Completed;
    session.storage.save_dialog_turn(path, &saved).await?;
    // The only turn file is at index 2, and the metadata update was lost.
    session
        .storage
        .update_session_metadata_if_present(path, CHAT, |metadata| {
            metadata.turn_count = 0;
            metadata.message_count = 0;
            metadata.last_turn = None;
            Ok(())
        })
        .await?;

    let file = session.layout().turn_path(CHAT, 2);
    let original = tokio::fs::read(&file).await?;
    tokio::fs::write(&file, b"{").await?;
    assert!(session.admit("saved", "earlier").await.is_err());
    assert!(!session.layout().turn_path(CHAT, 3).exists());
    tokio::fs::write(&file, original).await?;
    assert!(session.admit("saved", "earlier").await?.is_none());

    let receipt = session.admit("new", "hello").await?.unwrap();
    assert_eq!(receipt["turnIndex"], 3);
    let kept = session
        .storage
        .load_dialog_turn(path, CHAT, 2)
        .await?
        .unwrap();
    assert_eq!(kept.turn_id, "saved");
    assert_eq!(kept.user_message.content, "earlier");
    let added = session
        .storage
        .load_dialog_turn(path, CHAT, 3)
        .await?
        .unwrap();
    assert_eq!(added.turn_id, "new");
    Ok(())
}

#[tokio::test]
async fn repeated_prompts_without_output_are_split_by_the_prompts_saved_locally(
) -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    let path = session.root.path();
    session
        .save_interrupted_prompts(&["continue", "continue"])
        .await?;
    let mut agent = session.connect(None).await;
    // The agent replays no IDs: the first prompt was never answered.
    session
        .open_with(
            &mut agent,
            "session/load",
            &[
                ("user_message_chunk", "continue", None),
                ("user_message_chunk", "continue", None),
                ("agent_message_chunk", "answer", None),
            ],
        )
        .await?;
    let saved = session
        .storage
        .load_visible_session_turns(path, CHAT)
        .await?;
    assert_eq!(saved.len(), 2);
    assert!(saved[0].model_rounds.is_empty());
    assert_eq!(saved[1].model_rounds[0].text_items[0].content, "answer");
    Ok(())
}

#[tokio::test]
async fn adjacent_chunks_without_ids_join_when_a_saved_prompt_has_that_text() -> OpenBitFunResult<()>
{
    let session = TestSession::new().await?;
    let path = session.root.path();
    session
        .save_history(
            json!({"provider":"acp","acpRemoteSessionId":"original","acpHistoryGap":true}),
        )
        .await?;
    let mut known =
        super::super::transcript::start_turn(CHAT, "known", 0, "opencode", "earlier", None);
    known.status = TurnStatus::Error;
    session.storage.save_dialog_turn(path, &known).await?;
    // Without a usable saved prompt the legacy visible text still identifies this turn.
    let mut split = super::super::transcript::start_turn(
        CHAT,
        "split",
        1,
        "opencode",
        "hello again",
        Some(json!({"acpPromptText":7})),
    );
    split.status = TurnStatus::Error;
    session.storage.save_dialog_turn(path, &split).await?;
    let mut agent = session.connect(None).await;
    session
        .open_with(
            &mut agent,
            "session/load",
            &[
                (
                    "user_message_chunk",
                    "earlier",
                    Some(&known.user_message.id),
                ),
                ("agent_message_chunk", "first answer", None),
                ("user_message_chunk", "hello", None),
                ("user_message_chunk", " again", None),
                ("agent_message_chunk", "second answer", None),
            ],
        )
        .await?;
    let saved = session
        .storage
        .load_visible_session_turns(path, CHAT)
        .await?;
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[1].user_message.content, "hello again");
    assert_eq!(
        saved[1].model_rounds[0].text_items[0].content,
        "second answer"
    );
    assert!(!session.needs_history().await?);
    Ok(())
}

#[tokio::test]
async fn an_unrepairable_gap_is_not_replayed_again_until_local_history_changes(
) -> OpenBitFunResult<()> {
    let mut session = TestSession::new().await?;
    let path = session.root.path().to_owned();
    session
        .save_history(json!({"acpRemoteSessionId":"original","acpHistoryGap":true}))
        .await?;
    let mut known =
        super::super::transcript::start_turn(CHAT, "known", 0, "opencode", "earlier", None);
    known.status = TurnStatus::Error;
    session.storage.save_dialog_turn(&path, &known).await?;
    let known_id = known.user_message.id.clone();
    // Adjacent chunks the local history cannot place leave the gap in place.
    let ambiguous = [
        ("user_message_chunk", "earlier", Some(known_id.as_str())),
        ("agent_message_chunk", "recovered answer", None),
        ("user_message_chunk", "hello", None),
        ("user_message_chunk", " again", None),
    ];
    let mut agent = session.connect(None).await;
    session
        .open_with(&mut agent, "session/load", &ambiguous)
        .await?;
    assert!(session.needs_history().await?);

    // Nothing changed locally: reattach without decoding the history again.
    session.restart()?;
    let mut agent = session.connect(None).await;
    session.open_with(&mut agent, "session/resume", &[]).await?;
    assert!(session.needs_history().await?);

    // A new local turn changes the state, so the next restore tries again.
    let mut later =
        super::super::transcript::start_turn(CHAT, "later", 1, "opencode", "later", None);
    later.status = TurnStatus::Completed;
    session.storage.save_dialog_turn(&path, &later).await?;
    session.restart()?;
    let mut agent = session.connect(None).await;
    session.open_with(&mut agent, "session/load", &[]).await?;
    Ok(())
}

#[tokio::test]
async fn an_interrupted_turn_with_an_expanded_prompt_is_repaired_without_message_ids(
) -> OpenBitFunResult<()> {
    let mut session = TestSession::new().await?;
    let path = session.root.path().to_owned();
    session
        .save_history(json!({"acpRemoteSessionId":"original","provider":"acp"}))
        .await?;
    // The agent receives and replays more than the message the user sees.
    session
        .admit_with("interrupted", "@file context\nfirst", "first")
        .await?;
    for (name, mut payload) in [
        ("agentic://model-round-started", json!({"roundIndex":0})),
        ("agentic://text-chunk", json!({"text":"partial"})),
    ] {
        payload["sessionId"] = CHAT.into();
        payload["turnId"] = "interrupted".into();
        payload["roundId"] = "r".into();
        session
            .service
            .record_dialog_event(name, &mut payload)
            .unwrap();
    }
    session.restart()?;
    session
        .service
        .recover_stored_dialog_turn(&path, CHAT)
        .await?;
    assert!(session.needs_history().await?);

    let mut agent = session.connect(None).await;
    session
        .open_with(
            &mut agent,
            "session/load",
            &[
                ("user_message_chunk", "@file context\nfirst", None),
                ("agent_message_chunk", "partial recovered", None),
            ],
        )
        .await?;
    let repaired = session
        .storage
        .load_dialog_turn(&path, CHAT, 0)
        .await?
        .unwrap();
    assert_eq!(repaired.user_message.content, "first");
    assert_eq!(
        repaired.model_rounds[0].text_items[0].content,
        "partial recovered"
    );
    assert!(!session.needs_history().await?);
    Ok(())
}

#[tokio::test]
async fn a_remote_tail_that_repeats_old_prompt_text_keeps_the_gap() -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    let path = session.root.path();
    session
        .save_history(
            json!({"provider":"acp","acpRemoteSessionId":"original","acpHistoryGap":true}),
        )
        .await?;
    for (index, text) in ["继续", "检查", "看结果"].into_iter().enumerate() {
        let mut turn = super::super::transcript::start_turn(
            CHAT,
            &format!("turn-{index}"),
            index,
            "opencode",
            text,
            None,
        );
        turn.status = TurnStatus::Completed;
        session.storage.save_dialog_turn(path, &turn).await?;
    }
    let mut agent = session.connect(None).await;
    // The last prompt reuses words of earlier ones. Those words do not show where
    // it begins, and no saved prompt stands at that position.
    session
        .open_with(
            &mut agent,
            "session/load",
            &[
                ("user_message_chunk", "继续", None),
                ("agent_message_chunk", "a1", None),
                ("user_message_chunk", "检查", None),
                ("agent_message_chunk", "a2", None),
                ("user_message_chunk", "看结果", None),
                ("agent_message_chunk", "a3", None),
                ("user_message_chunk", "继续", None),
                ("user_message_chunk", "检查", None),
                ("agent_message_chunk", "a4", None),
            ],
        )
        .await?;
    let saved = session
        .storage
        .load_visible_session_turns(path, CHAT)
        .await?;
    assert_eq!(saved.len(), 3);
    assert!(session.needs_history().await?);
    Ok(())
}

#[tokio::test]
async fn stopping_a_turn_during_agent_startup_settles_before_initialize_returns(
) -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    let path = session.root.path();
    session.save_history(json!({})).await?;
    session.admit("turn", "hello").await?;
    let mut agent = Harness::new().await;
    let key = session_client_connection_id("opencode", CHAT);
    let client = test_client_connection(&key);
    // The agent process exists but has not answered initialize.
    *client.status.write().await = AcpClientStatus::Starting;
    session.service.clients.insert(key, client.clone());
    let mut cancelled = false;
    let mut worker = Box::pin(session.stream("turn", "hello", None, |event| {
        cancelled |= matches!(event, AcpClientStreamEvent::Cancelled);
        Ok(())
    }));
    assert!(futures::poll!(worker.as_mut()).is_pending());
    session
        .service
        .cancel_agent_session(
            "opencode",
            Some(path.display().to_string()),
            CHAT.into(),
            Some("turn"),
        )
        .await?;
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .expect("turn should settle while initialize is outstanding")?;
    assert!(cancelled);
    *client.connection.write().await = Some(agent.active.connection());
    *client.agent_capabilities.write().await = Some(AgentCapabilities::new());
    *client.status.write().await = AcpClientStatus::Running;
    let mut terminal = json!({"sessionId":CHAT,"turnId":"turn"});
    session
        .service
        .finish_dialog_turn(
            CHAT,
            Some(path),
            "agentic://dialog-turn-cancelled",
            &mut terminal,
        )
        .await?;
    let saved = session
        .storage
        .load_dialog_turn(path, CHAT, 0)
        .await?
        .unwrap();
    assert_eq!(saved.finish_reason.as_deref(), Some("acp_not_sent"));
    // The stopped prompt was never sent: the agent's first request is the next turn's.
    let (reply, next) = tokio::join!(session.prompt("next"), async {
        agent
            .reply("session/new", json!({"result":{"sessionId":"original"}}))
            .await;
        agent.finish_with_text("ok", "end_turn").await
    });
    assert_eq!(next["prompt"][0]["text"], "next");
    assert_eq!(reply?, "ok");
    Ok(())
}

#[tokio::test]
async fn a_replay_with_a_missing_head_never_places_a_reply_by_repeated_text() -> OpenBitFunResult<()>
{
    let user = "user_message_chunk";
    let agent = "agent_message_chunk";
    for (local, replay, replies) in [
        // Five turns can place AB either before reply-2 or before reply-3.
        (
            &["AB", "A", "B"][..],
            &[
                (user, "A"),
                (user, "B"),
                (agent, "reply-2"),
                (user, "A"),
                (user, "B"),
                (agent, "reply-3"),
                (user, "A"),
                (agent, "reply-4"),
                (user, "B"),
                (agent, "reply-5"),
            ][..],
            &[None, None, None][..],
        ),
        // The local B can be the first B, or the B following an unanswered A.
        (
            &["B", "AB"],
            &[
                (user, "B"),
                (agent, "reply-1"),
                (user, "A"),
                (user, "B"),
                (agent, "reply-2"),
                (user, "AB"),
                (agent, "reply-3"),
            ],
            &[None, None],
        ),
        // The agent stopped replaying just after the newer B. Its AB may follow on retry.
        (
            &["B", "AB"],
            &[
                (user, "B"),
                (agent, "older B"),
                (user, "AB"),
                (agent, "older AB"),
                (user, "B"),
                (agent, "newer B"),
            ],
            &[None, None],
        ),
        // Replacing the first B leaves just the placement inside the second run.
        (
            &["B", "AB"],
            &[
                (user, "older"),
                (agent, "reply-1"),
                (user, "A"),
                (user, "B"),
                (agent, "reply-2"),
                (user, "AB"),
                (agent, "reply-3"),
            ],
            &[Some("reply-2"), Some("reply-3")],
        ),
        // A saved prompt can also start at a later run and span both chunks.
        (
            &["ab"],
            &[
                (user, "older"),
                (agent, "reply-1"),
                (user, "a"),
                (user, "b"),
                (agent, "reply-2"),
            ],
            &[Some("reply-2")],
        ),
    ] {
        let session = TestSession::new().await?;
        session.save_interrupted_prompts(local).await?;
        let mut peer = session.connect(None).await;
        let replay: Vec<_> = replay
            .iter()
            .map(|&(kind, text)| (kind, text, None))
            .collect();
        session
            .open_with(&mut peer, "session/load", &replay)
            .await?;
        let saved = session
            .storage
            .load_visible_session_turns(session.root.path(), CHAT)
            .await?;
        assert_eq!(saved.len(), local.len());
        for ((turn, &prompt), &reply) in saved.iter().zip(local).zip(replies) {
            assert_eq!(turn.user_message.content, prompt);
            let contents: Vec<_> = turn
                .model_rounds
                .iter()
                .flat_map(|round| &round.text_items)
                .map(|item| item.content.as_str())
                .collect();
            assert_eq!(contents.as_slice(), reply.as_slice());
        }
        assert!(session.needs_history().await?);
    }
    Ok(())
}

#[tokio::test]
async fn incomplete_replay_is_retried_after_restart() -> OpenBitFunResult<()> {
    let user = "user_message_chunk";
    let agent = "agent_message_chunk";
    for (local, replay, first_len, replies, missing_head) in [
        // The agent has not replayed any history yet.
        (
            &["earlier"][..],
            &[(user, "earlier", None), (agent, "finished answer", None)][..],
            0,
            &["finished answer"][..],
            false,
        ),
        // The replay stops inside the first saved prompt.
        (
            &["abc"],
            &[
                (user, "a", None),
                (user, "b", None),
                (user, "c", None),
                (agent, "answer", None),
            ],
            2,
            &["answer"],
            false,
        ),
        // The replay stops exactly at the end of the first of two saved prompts.
        (
            &["ab", "cd"],
            &[
                (user, "a", None),
                (user, "b", None),
                (agent, "first answer", None),
                (user, "c", None),
                (user, "d", None),
                (agent, "second answer", None),
            ],
            2,
            &["first answer", "second answer"],
            false,
        ),
        // An earlier B/AB looks complete until the latest AB identifies its own turn.
        (
            &["B", "AB"],
            &[
                (user, "B", None),
                (agent, "older B", None),
                (user, "A", None),
                (user, "B", None),
                (agent, "older AB", None),
                (user, "B", None),
                (agent, "newer B", None),
                (user, "AB", Some("message-1")),
                (agent, "newer AB", None),
            ],
            7,
            &["newer B", "newer AB"],
            true,
        ),
    ] {
        let mut session = TestSession::new().await?;
        session.save_interrupted_prompts(local).await?;
        let mut peer = session.connect(None).await;
        session
            .open_with(&mut peer, "session/load", &replay[..first_len])
            .await?;
        assert!(session.needs_history().await?);
        let saved = session
            .storage
            .load_visible_session_turns(session.root.path(), CHAT)
            .await?;
        assert_eq!(saved.len(), local.len());
        assert!(saved.iter().all(|turn| turn.model_rounds.is_empty()));

        session.restart()?;
        let mut peer = session.connect(None).await;
        session.open_with(&mut peer, "session/load", replay).await?;
        let saved = session
            .storage
            .load_visible_session_turns(session.root.path(), CHAT)
            .await?;
        assert_eq!(saved.len(), local.len());
        for ((turn, &prompt), &reply) in saved.iter().zip(local).zip(replies) {
            assert_eq!(turn.user_message.content, prompt);
            assert_eq!(turn.model_rounds[0].text_items[0].content, reply);
        }
        assert_eq!(session.needs_history().await?, missing_head);
    }
    Ok(())
}

#[tokio::test]
async fn retrying_any_saved_turn_among_many_does_not_create_another() -> OpenBitFunResult<()> {
    let mut session = TestSession::new().await?;
    session.save_history(json!({"provider": "acp"})).await?;
    for index in 0..9 {
        let mut turn = super::super::transcript::start_turn(
            CHAT,
            &format!("turn-{index}"),
            index,
            "opencode",
            "hello",
            None,
        );
        turn.status = TurnStatus::Completed;
        session
            .storage
            .save_dialog_turn(session.root.path(), &turn)
            .await?;
    }
    // Restart with all nine catalog entries missing their turn IDs, then retry the oldest.
    tokio::fs::remove_file(session.layout().turn_catalog_path(CHAT)).await?;
    session.restart()?;
    let file = session.layout().turn_path(CHAT, 0);
    let original = tokio::fs::read(&file).await?;
    tokio::fs::write(&file, b"{").await?;
    assert!(session.admit("turn-0", "hello").await.is_err());
    assert!(!session.layout().turn_path(CHAT, 9).exists());
    tokio::fs::write(&file, original).await?;
    assert!(session.admit("turn-0", "hello").await?.is_none());
    assert!(session
        .storage
        .load_dialog_turn(session.root.path(), CHAT, 9)
        .await?
        .is_none());
    Ok(())
}

#[tokio::test]
async fn retrying_legacy_in_progress_turn_keeps_saved_output() -> OpenBitFunResult<()> {
    let mut session = TestSession::new().await?;
    session.save_history(json!({"provider":"acp"})).await?;
    let mut turn = super::super::transcript::start_turn(CHAT, "turn", 0, "opencode", "hello", None);
    super::super::transcript::start_round(&mut turn, "round", 0, 1);
    super::super::transcript::append_text(&mut turn.model_rounds[0], "partial", false, 2);
    session
        .storage
        .save_dialog_turn(session.root.path(), &turn)
        .await?;

    session.restart()?;
    assert!(session.admit("turn", "hello").await?.is_none());

    let saved = session
        .storage
        .load_dialog_turn(session.root.path(), CHAT, 0)
        .await?
        .unwrap();
    assert_eq!(saved.model_rounds[0].text_items[0].content, "partial");
    assert_ne!(saved.status, TurnStatus::InProgress);
    Ok(())
}

#[tokio::test]
async fn releasing_before_or_during_worker_startup_never_sends_the_admitted_prompt(
) -> OpenBitFunResult<()> {
    for worker_started in [false, true] {
        let session = TestSession::new().await?;
        session.save_history(json!({})).await?;
        assert!(session.admit("turn", "hello").await?.is_some());
        let mut agent = Harness::new().await;
        let connection = agent.active.connection();
        let key = session_client_connection_id("opencode", CHAT);
        let client = test_client_connection(&key);
        *client.status.write().await = AcpClientStatus::Starting;
        session.service.clients.insert(key, client.clone());
        let mut cancelled = false;
        let mut worker = Box::pin(session.stream("turn", "hello", None, |event| {
            cancelled |= matches!(event, AcpClientStreamEvent::Cancelled);
            Ok(())
        }));
        if worker_started {
            // The worker subscribed to this turn, but the agent has not initialized.
            assert!(futures::poll!(worker.as_mut()).is_pending());
        }
        session.service.release_openbitfun_session(CHAT).await;
        *client.connection.write().await = Some(connection.clone());
        *client.agent_capabilities.write().await = Some(AgentCapabilities::new());
        *client.status.write().await = AcpClientStatus::Running;

        tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .expect("released turn should settle")?;
        assert!(cancelled);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if client.sessions.is_empty() && client.cancel_handles.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("released startup should not leave ACP session handles");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), agent.request())
                .await
                .is_err()
        );
    }
    Ok(())
}

#[tokio::test]
async fn releasing_while_the_admitted_turn_loads_never_sends_its_prompt() -> OpenBitFunResult<()> {
    let session = TestSession::new().await?;
    session
        .save_history(json!({"acpRemoteSessionId":"original","acpRestoreRequired":true}))
        .await?;
    assert!(session.admit("turn", "hello").await?.is_some());
    let mut agent = session.connect(None).await;
    let mut cancelled = false;
    let mut worker = Box::pin(session.stream("turn", "hello", None, |event| {
        cancelled |= matches!(event, AcpClientStreamEvent::Cancelled);
        Ok(())
    }));
    let load = tokio::select! {
        request = agent.request() => request,
        result = &mut worker => panic!("turn settled before session/load: {result:?}"),
    };
    assert_eq!(load["method"], "session/load");
    assert_eq!(load["params"]["sessionId"], "original");
    let release = session.service.release_openbitfun_session(CHAT);
    tokio::pin!(release);
    // The pending send must settle even while release is blocked on the load.
    assert!(futures::poll!(&mut release).is_pending());
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .expect("release should cancel the turn before the agent answers session/load")?;
    assert!(cancelled);
    agent
        .send(json!({"jsonrpc":"2.0","id":load["id"],"result":{}}))
        .await;
    assert!(release.await);
    let request = agent.request().await;
    assert_eq!(request["method"], "session/cancel", "{request}");
    assert_eq!(request["params"]["sessionId"], "original");
    agent
        .active
        .connection()
        .send_notification(CancelNotification::new("wire-fence"))
        .unwrap();
    let next = agent.request().await;
    assert_eq!(next["method"], "session/cancel", "{next}");
    assert_eq!(next["params"]["sessionId"], "wire-fence");
    Ok(())
}
