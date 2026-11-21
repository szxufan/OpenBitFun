use super::super::test_support::test_ai_config;
use super::super::test_support::test_coordinator;
#[cfg(feature = "external-sources")]
use super::super::test_support::test_persistent_coordinator;
use super::super::{
    turn_review_manifest_for_agent, ConversationCoordinator, DialogSubmissionPolicy,
    DialogTriggerSource,
};
use crate::agentic::core::{
    Message, MessageContent, SessionAgentRouteOwner, SessionConfig, SessionContinuationPolicy,
    SessionKind, SessionState,
};
use crate::agentic::persistence::PersistenceManager;
use crate::agentic::session::TEST_MODEL_RESOLUTION_AI_CONFIG;
use crate::infrastructure::PathManager;
use crate::service::config::types::AIModelConfig;
use std::sync::Arc;

#[cfg(feature = "external-sources")]
#[tokio::test]
async fn explicit_agent_change_switches_owner_but_case_variant_does_not() {
    let (_coordinator, session_manager) = test_persistent_coordinator();
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let session_id = format!("external-to-local-{}", uuid::Uuid::new_v4());
    let external_agent_id = format!("external-profile-{}", uuid::Uuid::new_v4());
    session_manager
        .create_session_with_id(
            Some(session_id.clone()),
            "External to local".to_string(),
            external_agent_id.clone(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("create session");
    session_manager
        .update_session_agent_binding(
            &session_id,
            &external_agent_id,
            SessionAgentRouteOwner::External,
            Some("test:external".to_string()),
        )
        .await
        .expect("persist external route owner");
    let session = session_manager
        .get_session(&session_id)
        .expect("session remains loaded");
    let workspace = ConversationCoordinator::build_workspace_binding(&session.config).await;

    let binding =
        ConversationCoordinator::resolve_session_agent_for_turn(&session, "Standard", &workspace)
            .await
            .expect("explicitly selected local mode should resolve independently of the old owner");

    assert_eq!(binding.runtime_agent_key, "Standard");
    assert_eq!(binding.route_owner, SessionAgentRouteOwner::Local);

    session_manager
        .update_session_agent_binding(
            &session_id,
            "STANDARD",
            SessionAgentRouteOwner::External,
            Some("test:external".to_string()),
        )
        .await
        .expect("persist case-variant external route owner");
    let case_variant_session = session_manager
        .get_session(&session_id)
        .expect("case-variant session remains loaded");
    let error = match ConversationCoordinator::resolve_session_agent_for_turn(
        &case_variant_session,
        "Standard",
        &workspace,
    )
    .await
    {
        Ok(_) => panic!("case variants of the same external identity must remain fail-closed"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("candidate_unavailable"));
}

#[tokio::test]
async fn legacy_imported_primary_branches_resolve_real_turn_modes() {
    use crate::agentic::persistence::SessionBranchRequest;
    use openbitfun_services_core::session::{
        DialogTurnData, DialogTurnKind, OfflineSessionBundle, OfflineSessionImportStore,
        SessionBranchBoundary, SessionMetadata, TurnStatus, UserMessageData,
    };

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let workspace =
        crate::service::workspace::legacy_compat::register_local_fixture_blocking(&project);
    let paths = Arc::new(PathManager::with_user_root_for_tests(
        temp.path().join("user"),
    ));
    let root = paths.project_sessions_dir(&workspace.root_path);
    let persistence = PersistenceManager::new(paths).unwrap();
    let store = OfflineSessionImportStore::new(&root);
    for ecosystem in ["codex", "zcode"] {
        let id = format!("legacy-{ecosystem}-branch");
        let turn_id = format!("{id}-turn-0");
        let metadata: SessionMetadata = serde_json::from_value(serde_json::json!({
                "session_id": id, "session_name": "Existing native branch", "agent_type": "primary", "model_name": "primary",
                "created_at": 1, "last_active_at": 2, "turn_count": 1, "message_count": 1, "tool_call_count": 0, "status": "completed",
                "workspace_id": workspace.id, "workspace_path": workspace.root_path,
                "project_workspace_id": workspace.id, "project_workspace_path": workspace.root_path,
                "custom_metadata": {"externalBranchOf": {"sessionId": "source-view", "ecosystemId": ecosystem, "sourceSessionId": "source", "contentVersion": "pinned"}}
            })).unwrap();
        let mut turn = DialogTurnData::new_with_kind(
            DialogTurnKind::UserDialog,
            turn_id.clone(),
            0,
            id.clone(),
            Some("primary".into()),
            UserMessageData {
                id: format!("{id}-user"),
                content: "Preserved imported question".into(),
                timestamp: 1,
                metadata: None,
            },
        );
        turn.status = TurnStatus::Completed;
        store
            .write_bundle_atomic(&OfflineSessionBundle {
                metadata,
                turns: vec![turn],
            })
            .await
            .unwrap();
        let metadata_path = root.join(&id).join("metadata.json");
        let before = std::fs::read(&metadata_path).unwrap();
        let restored = persistence
            .load_session(&workspace.root_path, &id)
            .await
            .unwrap();

        let admitted = ConversationCoordinator::resolve_session_agent_for_turn(
            &restored,
            &restored.agent_type,
            &None,
        )
        .await
        .expect("legacy native branches must pass the same mode admission as a message send");
        assert_eq!(admitted.runtime_agent_key, "Standard");
        assert_eq!(restored.config.model_id.as_deref(), Some("primary"));
        assert_eq!(std::fs::read(&metadata_path).unwrap(), before);

        let branch = persistence
            .branch_session(
                &workspace.root_path,
                &SessionBranchRequest {
                    source_session_id: id.clone(),
                    source_turn_id: turn_id,
                    boundary: SessionBranchBoundary::ThroughTurn,
                },
            )
            .await
            .unwrap();
        let continued = persistence
            .load_session(&workspace.root_path, &branch.session_id)
            .await
            .unwrap();
        let admitted = ConversationCoordinator::resolve_session_agent_for_turn(
            &continued,
            &continued.agent_type,
            &None,
        )
        .await
        .expect("forking an existing legacy branch must retain a valid runtime mode");
        assert_eq!(admitted.runtime_agent_key, "Standard");
        let copied = store
            .load_bundle(&branch.session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            copied.turns[0].user_message.content,
            "Preserved imported question"
        );
        assert_eq!(copied.turns[0].agent_type.as_deref(), Some("primary"));
    }
}

#[tokio::test]
async fn subagent_conversation_resolves_legacy_children_on_local_and_remote_workspaces() {
    for agent_type in ["GeneralPurpose", "Explore", "Standard"] {
        let mut child = crate::agentic::core::Session::new(
            "Legacy child".to_string(),
            agent_type.to_string(),
            SessionConfig::default(),
        );
        child.kind = SessionKind::Subagent;
        let mut legacy = serde_json::to_value(&child).expect("serialize legacy child");
        let config = legacy["config"].as_object_mut().expect("config object");
        for key in [
            "continuation_policy",
            "agent_route_owner",
            "agent_route_key",
        ] {
            config.remove(key);
        }
        let child: crate::agentic::core::Session =
            serde_json::from_value(legacy).expect("read old child without route fields");
        assert_eq!(
            child.config.continuation_policy,
            SessionContinuationPolicy::Reusable
        );
        assert_eq!(
            child.config.agent_route_owner,
            SessionAgentRouteOwner::Local
        );

        let remote = crate::agentic::workspace::WorkspaceBinding::new_remote(
            None,
            std::path::PathBuf::from("/remote/workspace"),
            "remote-child-host".to_string(),
            "Remote host".to_string(),
            openbitfun_services_core::workspace_identity::WorkspaceSessionIdentity {
                workspace_kind: openbitfun_core_types::WorkspaceKind::Remote,
                hostname: "remote-host".to_string(),
                logical_workspace_path: "/remote/workspace".to_string(),
                remote_connection_id: Some("remote-child-host".to_string()),
            },
        );
        for workspace in [None, Some(remote)] {
            let binding = ConversationCoordinator::resolve_session_agent_for_turn(
                &child, agent_type, &workspace,
            )
            .await
            .expect("legacy reusable child must resolve its existing local agent");
            assert_eq!(binding.runtime_agent_key, agent_type);
            assert_eq!(binding.route_owner, SessionAgentRouteOwner::Local);
        }
        let round_trip: crate::agentic::core::Session =
            serde_json::from_value(serde_json::to_value(&child).expect("write upgraded child"))
                .expect("read upgraded child");
        assert_eq!(round_trip.kind, child.kind);
        assert_eq!(round_trip.agent_type, agent_type);
        assert_eq!(
            round_trip.config.continuation_policy,
            child.config.continuation_policy
        );
    }
}

#[tokio::test]
async fn subagent_conversation_direct_turn_preserves_context_and_rejects_fresh_only() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace = tempfile::tempdir().expect("child workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let workspace_path = workspace.path().to_string_lossy().into_owned();
    let ai_config = test_ai_config(vec![AIModelConfig {
        id: "child-model".to_string(),
        name: "Child model".to_string(),
        provider: "openai".to_string(),
        model_name: "test-model".to_string(),
        enabled: true,
        ..AIModelConfig::default()
    }]);
    for agent_type in ["GeneralPurpose", "Explore"] {
        for policy in [
            SessionContinuationPolicy::Reusable,
            SessionContinuationPolicy::FreshOnly,
        ] {
            let child = coordinator
                .create_hidden_subagent_session(
                    None,
                    "Delegated child".to_string(),
                    agent_type.to_string(),
                    SessionConfig {
                        tool_presentation_mode: Default::default(),
                        workspace_path: Some(workspace_path.clone()),
                        model_id: Some("child-model".to_string()),
                        continuation_policy: policy,
                        ..Default::default()
                    },
                    Some("parent-session".to_string()),
                )
                .await
                .expect("create delegated child");
            session_manager
                .add_message(
                    &child.session_id,
                    Message::user("Initial delegated task".to_string()),
                )
                .await
                .expect("seed delegated context");
            let turn_id = format!("child-follow-up-{}", uuid::Uuid::new_v4());
            let result = TEST_MODEL_RESOLUTION_AI_CONFIG
                .scope(
                    ai_config.clone(),
                    coordinator.start_dialog_turn(
                        child.session_id.clone(),
                        "Continue the task".to_string(),
                        None,
                        Some(turn_id.clone()),
                        agent_type.to_string(),
                        Some(workspace_path.clone()),
                        None,
                        None,
                        DialogSubmissionPolicy::for_source(DialogTriggerSource::DesktopUi),
                        None,
                    ),
                )
                .await;
            let updated = session_manager
                .get_session(&child.session_id)
                .expect("child remains loaded");
            assert_eq!(updated.agent_type, agent_type);
            assert_eq!(updated.kind, SessionKind::Subagent);
            assert_eq!(updated.created_by, child.created_by);
            assert_eq!(updated.config.model_id.as_deref(), Some("child-model"));
            if policy == SessionContinuationPolicy::FreshOnly {
                assert!(result
                    .expect_err("fresh-only follow-up must fail")
                    .to_string()
                    .contains("subagent_follow_up_unsupported"));
                assert!(updated.dialog_turn_ids.is_empty());
                assert_eq!(updated.config.agent_route_key, child.config.agent_route_key);
                assert!(matches!(updated.state, SessionState::Idle));
            } else {
                result.expect("a reusable child must admit a direct user turn");
                assert_eq!(updated.dialog_turn_ids, vec![turn_id.clone()]);
                assert_eq!(
                    updated.config.agent_route_owner,
                    SessionAgentRouteOwner::Local
                );
                assert!(updated.config.agent_route_key.is_some());
                let messages = session_manager
                    .get_context_messages(&child.session_id)
                    .await
                    .expect("child context");
                for expected in ["Initial delegated task", "Continue the task"] {
                    assert!(messages.iter().any(|message| {
                            matches!(&message.content, MessageContent::Text(text) if text == expected)
                        }), "child context must contain {expected}");
                }
                let _ = coordinator
                    .cancel_dialog_turn(&child.session_id, &turn_id)
                    .await;
            }
        }
    }
}

#[tokio::test]
async fn subagent_conversation_keeps_primary_fresh_only_and_route_guards() {
    let mut child = crate::agentic::core::Session::new(
        "Child".to_string(),
        "GeneralPurpose".to_string(),
        SessionConfig::default(),
    );
    child.kind = SessionKind::Subagent;
    assert!(
        ConversationCoordinator::resolve_session_agent_for_turn(&child, "Explore", &None)
            .await
            .is_err(),
        "a different subagent must not be selected as a mode"
    );

    child.config.agent_route_key = Some("wrong-source".to_string());
    let error =
        ConversationCoordinator::resolve_session_agent_for_turn(&child, "GeneralPurpose", &None)
            .await
            .err()
            .expect("a changed source must be rejected");
    assert!(error.to_string().contains("candidate_unavailable"));
    child.config.agent_route_key = None;
    child.config.agent_route_owner = SessionAgentRouteOwner::External;
    assert!(
        ConversationCoordinator::resolve_session_agent_for_turn(&child, "GeneralPurpose", &None,)
            .await
            .is_err(),
        "an external owner must not fall back to a local child"
    );

    child.config.agent_route_owner = SessionAgentRouteOwner::Local;
    child.config.continuation_policy = SessionContinuationPolicy::FreshOnly;
    let error =
        ConversationCoordinator::resolve_session_agent_for_turn(&child, "GeneralPurpose", &None)
            .await
            .err()
            .expect("fresh-only children must remain read-only");
    assert!(error.to_string().contains("subagent_follow_up_unsupported"));

    child.config.continuation_policy = SessionContinuationPolicy::Reusable;
    child.kind = SessionKind::Standard;
    let error =
        ConversationCoordinator::resolve_session_agent_for_turn(&child, "GeneralPurpose", &None)
            .await
            .err()
            .expect("subagents must not become main-session modes");
    assert!(error
        .to_string()
        .contains("Unknown session mode: GeneralPurpose"));
}

#[tokio::test]
async fn review_agent_child_sessions_create_successfully() {
    let (coordinator, _session_manager) = test_coordinator();

    for agent_type in ["CodeReview", "DeepReview"] {
        let workspace = tempfile::tempdir().expect("review workspace");
        crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
        let session = coordinator
            .create_session_with_workspace(
                None,
                format!("Review child: {agent_type}"),
                agent_type.to_string(),
                SessionConfig {
                    tool_presentation_mode: Default::default(),
                    workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                    ..Default::default()
                },
                workspace.path().to_string_lossy().into_owned(),
            )
            .await
            .unwrap_or_else(|error| {
                panic!("{agent_type} review child session must create: {error}")
            });
        assert_eq!(session.agent_type, agent_type);
    }
}

#[tokio::test]
async fn review_fixer_turn_is_admitted_after_updating_a_deep_review_session_binding() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace = tempfile::tempdir().expect("review workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let workspace_path = workspace.path().to_string_lossy().into_owned();
    let session = session_manager
        .create_session(
            "Deep review remediation".to_string(),
            "DeepReview".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace_path.clone()),
                model_id: Some("review-model".to_string()),
                enable_tools: true,
                ..Default::default()
            },
        )
        .await
        .expect("DeepReview session should be created");
    let ai_config = test_ai_config(vec![AIModelConfig {
        id: "review-model".to_string(),
        name: "Review model".to_string(),
        provider: "openai".to_string(),
        model_name: "test-model".to_string(),
        enabled: true,
        ..AIModelConfig::default()
    }]);
    let fix_turn_id = "review-fix-turn";
    TEST_MODEL_RESOLUTION_AI_CONFIG
        .scope(
            ai_config,
            coordinator.start_dialog_turn(
                session.session_id.clone(),
                "fix selected findings".to_string(),
                Some("fix selected findings".to_string()),
                Some(fix_turn_id.to_string()),
                "ReviewFixer".to_string(),
                Some(workspace_path),
                None,
                None,
                DialogSubmissionPolicy::for_source(DialogTriggerSource::DesktopApi),
                None,
            ),
        )
        .await
        .expect("ReviewFixer turn should pass admission after the intentional binding update");

    let updated = session_manager
        .get_session(&session.session_id)
        .expect("review session should remain loaded");
    assert_eq!(updated.agent_type, "ReviewFixer");
    assert_eq!(session_manager.get_turn_count(&session.session_id), 1);

    let _ = coordinator
        .cancel_dialog_turn(&session.session_id, fix_turn_id)
        .await;
}

#[test]
fn turn_review_manifest_is_ignored_for_ordinary_agents() {
    let metadata = serde_json::json!({
        "deepReviewRunManifest": { "reviewTargetEvidence": { "version": 1 } }
    });

    assert!(turn_review_manifest_for_agent(Some(&metadata), "Standard").is_none());
    assert!(turn_review_manifest_for_agent(Some(&metadata), "CodeReview").is_some());
    assert!(turn_review_manifest_for_agent(Some(&metadata), "DeepReview").is_some());
}
