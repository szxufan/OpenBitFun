use super::support::{test_manager, TestWorkspace};
use crate::agentic::core::SessionConfig;
use crate::agentic::persistence::PersistenceManager;
use crate::service::session::{
    SessionKind, SessionMetadata, SessionRelationship, SessionRelationshipKind,
};
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn ephemeral_child_session_is_kept_in_memory_without_persisting() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());

    let session = manager
        .create_session_with_id_and_details(
            Some(Uuid::new_v4().to_string()),
            "Side thread".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
            Some("session-parent".to_string()),
            SessionKind::EphemeralChild,
        )
        .await
        .expect("ephemeral child session should create");

    assert!(manager.get_session(&session.session_id).is_some());
    assert!(persistence_manager
        .load_session_metadata(workspace.path(), &session.session_id)
        .await
        .expect("metadata lookup should succeed")
        .is_none());
    assert_eq!(
        manager
            .persistent_model_exchange_trace_dir(&session.session_id)
            .await,
        None
    );
}

#[tokio::test]
async fn persisted_session_uses_session_local_model_exchange_trace_dir() {
    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let persistence_manager =
        Arc::new(PersistenceManager::new(path_manager.clone()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);

    let session = manager
        .create_session_with_id_and_details(
            Some(Uuid::new_v4().to_string()),
            "Main thread".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
            None,
            SessionKind::Standard,
        )
        .await
        .expect("standard session should create");

    assert_eq!(
        manager
            .persistent_model_exchange_trace_dir(&session.session_id)
            .await,
        Some(
            path_manager
                .project_sessions_dir(workspace.path())
                .join(&session.session_id)
                .join("request-traces")
        )
    );
}

#[tokio::test]
async fn persist_session_lineage_updates_structured_relationship_and_clears_legacy_projection() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());

    let session = manager
        .create_session_with_id_and_details(
            Some(Uuid::new_v4().to_string()),
            "Review child".to_string(),
            "CodeReview".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
            Some("session-parent".to_string()),
            SessionKind::Standard,
        )
        .await
        .expect("session should create");

    manager
        .merge_session_custom_metadata(
            &session.session_id,
            json!({
                "kind": "review",
                "parentSessionId": "stale-parent",
                "parentRequestId": "stale-request",
                "parentDialogTurnId": "stale-turn",
                "parentTurnIndex": 1,
                "parentToolCallId": "stale-tool",
                "subagentType": "stale-subagent",
                "preservedKey": "preserved-value",
            }),
        )
        .await
        .expect("legacy compatibility metadata should seed");

    manager
        .persist_session_lineage(
            &session.session_id,
            SessionRelationship {
                kind: Some(SessionRelationshipKind::DeepReview),
                parent_session_id: Some("parent-1".to_string()),
                parent_request_id: Some("request-1".to_string()),
                parent_dialog_turn_id: Some("turn-2".to_string()),
                parent_turn_index: Some(2),
                parent_tool_call_id: None,
                subagent_type: None,
                continuation_policy: None,
            },
        )
        .await
        .expect("lineage should persist");

    let metadata = persistence_manager
        .load_session_metadata(workspace.path(), &session.session_id)
        .await
        .expect("metadata lookup should succeed")
        .expect("metadata should exist");

    assert_eq!(
        metadata.relationship,
        Some(SessionRelationship {
            kind: Some(SessionRelationshipKind::DeepReview),
            parent_session_id: Some("parent-1".to_string()),
            parent_request_id: Some("request-1".to_string()),
            parent_dialog_turn_id: Some("turn-2".to_string()),
            parent_turn_index: Some(2),
            parent_tool_call_id: None,
            subagent_type: None,
            continuation_policy: None,
        })
    );

    let custom_metadata = metadata
        .custom_metadata
        .expect("non-lineage custom metadata should remain");
    assert_eq!(custom_metadata["preservedKey"], "preserved-value");
    assert!(custom_metadata.get("kind").is_none());
    assert!(custom_metadata.get("parentSessionId").is_none());
    assert!(custom_metadata.get("parentRequestId").is_none());
    assert!(custom_metadata.get("parentDialogTurnId").is_none());
    assert!(custom_metadata.get("parentTurnIndex").is_none());
    assert!(custom_metadata.get("parentToolCallId").is_none());
    assert!(custom_metadata.get("subagentType").is_none());
}

#[tokio::test]
async fn collect_hidden_subagent_cascade_for_parent_turns_returns_post_order_matches() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());

    let mut matched_root = SessionMetadata::new(
        "child-root".to_string(),
        "Subagent: root".to_string(),
        "Explore".to_string(),
        "model".to_string(),
    );
    matched_root.session_kind = SessionKind::Subagent;
    matched_root.relationship = Some(SessionRelationship {
        kind: Some(SessionRelationshipKind::Subagent),
        parent_session_id: Some("parent-session".to_string()),
        parent_request_id: None,
        parent_dialog_turn_id: Some("turn-2".to_string()),
        parent_turn_index: Some(2),
        parent_tool_call_id: Some("tool-1".to_string()),
        subagent_type: Some("Explore".to_string()),
        continuation_policy: None,
    });
    persistence_manager
        .save_session_metadata(workspace.path(), &matched_root)
        .await
        .expect("matched root should save");

    let mut matched_grandchild = SessionMetadata::new(
        "grandchild".to_string(),
        "Subagent: grandchild".to_string(),
        "Explore".to_string(),
        "model".to_string(),
    );
    matched_grandchild.session_kind = SessionKind::Subagent;
    matched_grandchild.relationship = Some(SessionRelationship {
        kind: Some(SessionRelationshipKind::Subagent),
        parent_session_id: Some("child-root".to_string()),
        parent_request_id: None,
        parent_dialog_turn_id: Some("child-turn".to_string()),
        parent_turn_index: None,
        parent_tool_call_id: Some("tool-child".to_string()),
        subagent_type: Some("Explore".to_string()),
        continuation_policy: None,
    });
    persistence_manager
        .save_session_metadata(workspace.path(), &matched_grandchild)
        .await
        .expect("grandchild should save");

    let mut unmatched_root = SessionMetadata::new(
        "child-other-turn".to_string(),
        "Subagent: other turn".to_string(),
        "Explore".to_string(),
        "model".to_string(),
    );
    unmatched_root.session_kind = SessionKind::Subagent;
    unmatched_root.relationship = Some(SessionRelationship {
        kind: Some(SessionRelationshipKind::Subagent),
        parent_session_id: Some("parent-session".to_string()),
        parent_request_id: None,
        parent_dialog_turn_id: Some("turn-1".to_string()),
        parent_turn_index: Some(1),
        parent_tool_call_id: Some("tool-2".to_string()),
        subagent_type: Some("Explore".to_string()),
        continuation_policy: None,
    });
    persistence_manager
        .save_session_metadata(workspace.path(), &unmatched_root)
        .await
        .expect("unmatched root should save");

    let mut visible_review_child = SessionMetadata::new(
        "review-child".to_string(),
        "Review child".to_string(),
        "DeepReview".to_string(),
        "model".to_string(),
    );
    visible_review_child.relationship = Some(SessionRelationship {
        kind: Some(SessionRelationshipKind::DeepReview),
        parent_session_id: Some("parent-session".to_string()),
        parent_request_id: None,
        parent_dialog_turn_id: Some("turn-2".to_string()),
        parent_turn_index: Some(2),
        parent_tool_call_id: None,
        subagent_type: None,
        continuation_policy: None,
    });
    persistence_manager
        .save_session_metadata(workspace.path(), &visible_review_child)
        .await
        .expect("visible review child should save");

    let matched_turn_ids = HashSet::from(["turn-2".to_string()]);
    let cascade = manager
        .collect_hidden_subagent_cascade_for_parent_turns(
            workspace.path(),
            "parent-session",
            &matched_turn_ids,
        )
        .await
        .expect("cascade lookup should succeed");

    assert_eq!(
        cascade,
        vec!["grandchild".to_string(), "child-root".to_string()]
    );
}
