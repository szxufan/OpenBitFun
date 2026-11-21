use super::support::{test_manager, TestWorkspace};
use crate::agentic::core::SessionConfig;
use crate::agentic::persistence::PersistenceManager;
use std::sync::Arc;

#[tokio::test]
async fn edit_constraints_are_cached_and_inherited_by_forked_children() {
    use crate::agentic::execution::edit_constraint_guard::{
        ConstraintExtractionRecord, ConstraintMatcher, ConstraintOperationScope, ConstraintSource,
        ExtractedConstraint, ExtractionStatus, ModelExtractionStatus,
    };

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = test_manager(persistence_manager);

    // Uncached: distinct from "cached but empty".
    assert_eq!(manager.edit_constraints("parent-session"), None);

    let constraints = vec![ExtractedConstraint {
        id: "test-files".to_string(),
        description: "don't modify test files".to_string(),
        operation_scope: ConstraintOperationScope::All,
        matcher: ConstraintMatcher::TestFiles,
        source: ConstraintSource::Legacy,
        source_text: None,
    }];
    manager
        .remember_edit_constraint_extraction(
            "parent-session",
            ConstraintExtractionRecord {
                message_sha256: "message-hash".to_string(),
                dialog_turn_id: Some("turn-1".to_string()),
                status: ExtractionStatus::Extracted,
                constraints: constraints.clone(),
                deterministic_constraint_count: 0,
                model_attempts: 1,
                active_constraint_ids: Vec::new(),
                revocation_authorized: true,
                model_status: ModelExtractionStatus::Parsed,
                model_constraints: constraints.clone(),
                model_revocations: Vec::new(),
                revoked_constraint_ids: Vec::new(),
                unmatched_revocation_ids: Vec::new(),
                input_chars: 10,
                prompt_chars: 10,
                input_truncated: false,
                latency_ms: 1,
                extracted_at_ms: 1,
                failure: None,
                response_excerpt: None,
            },
        )
        .await;
    assert_eq!(
        manager.edit_constraints("parent-session"),
        Some(constraints.clone())
    );
    manager
        .remember_edit_constraint_agent_created_paths(
            "parent-session",
            vec!["tests/parent_repro.rs".to_string()],
            "turn-1",
        )
        .await;

    // A forked child with no prior extraction inherits the parent's list.
    assert_eq!(manager.edit_constraints("child-session"), None);
    manager
        .seed_forked_edit_constraints("parent-session", "child-session")
        .await;
    assert_eq!(
        manager.edit_constraints("child-session"),
        Some(constraints.clone())
    );
    manager
        .rollback_edit_constraint_state_to_turns("child-session", &std::collections::HashSet::new())
        .await;
    let child_state = manager
        .edit_constraint_state("child-session")
        .expect("forked state after rollback");
    assert_eq!(child_state.constraints, constraints);
    assert_eq!(
        child_state.agent_created_paths,
        vec!["tests/parent_repro.rs".to_string()]
    );

    // Seeding from a parent with no cached constraints is a no-op, not a panic.
    manager
        .seed_forked_edit_constraints("no-such-parent", "another-child")
        .await;
    assert_eq!(manager.edit_constraints("another-child"), None);
}

#[tokio::test]
async fn edit_constraint_state_persists_across_session_restore() {
    use crate::agentic::execution::edit_constraint_guard::{
        ConstraintExtractionRecord, ConstraintMatcher, ConstraintOperationScope,
        ConstraintRevocation, ConstraintSource, ExtractedConstraint, ExtractionStatus,
        ModelExtractionStatus, EDIT_CONSTRAINT_METADATA_KEY,
    };

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Edit constraint persistence".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should be created");
    let constraint = ExtractedConstraint {
        id: "deterministic:test_files".to_string(),
        description: "do not modify tests".to_string(),
        operation_scope: ConstraintOperationScope::All,
        matcher: ConstraintMatcher::TestFiles,
        source: ConstraintSource::Deterministic,
        source_text: Some("Do not modify tests.".to_string()),
    };
    manager
        .remember_edit_constraint_extraction(
            &session.session_id,
            ConstraintExtractionRecord {
                message_sha256: "message-hash".to_string(),
                dialog_turn_id: Some("turn-1".to_string()),
                status: ExtractionStatus::Extracted,
                constraints: vec![constraint.clone()],
                deterministic_constraint_count: 1,
                model_attempts: 0,
                active_constraint_ids: Vec::new(),
                revocation_authorized: true,
                model_status: ModelExtractionStatus::NotRun,
                model_constraints: Vec::new(),
                model_revocations: Vec::new(),
                revoked_constraint_ids: Vec::new(),
                unmatched_revocation_ids: Vec::new(),
                input_chars: 20,
                prompt_chars: 20,
                input_truncated: false,
                latency_ms: 1,
                extracted_at_ms: 1,
                failure: None,
                response_excerpt: None,
            },
        )
        .await;
    manager
        .remember_edit_constraint_agent_created_paths(
            &session.session_id,
            vec!["tests/temporary-repro.rs".to_string()],
            "turn-1",
        )
        .await;
    manager
        .remember_edit_constraint_extraction(
            &session.session_id,
            ConstraintExtractionRecord {
                message_sha256: "relaxation-hash".to_string(),
                dialog_turn_id: Some("turn-2".to_string()),
                status: ExtractionStatus::Extracted,
                constraints: Vec::new(),
                deterministic_constraint_count: 0,
                model_attempts: 1,
                active_constraint_ids: vec![constraint.id.clone()],
                revocation_authorized: true,
                model_status: ModelExtractionStatus::Parsed,
                model_constraints: Vec::new(),
                model_revocations: vec![ConstraintRevocation {
                    constraint_id: constraint.id.clone(),
                    description: "tests may be modified now".to_string(),
                }],
                revoked_constraint_ids: vec![constraint.id.clone()],
                unmatched_revocation_ids: Vec::new(),
                input_chars: 24,
                prompt_chars: 24,
                input_truncated: false,
                latency_ms: 1,
                extracted_at_ms: 2,
                failure: None,
                response_excerpt: None,
            },
        )
        .await;

    let metadata = persistence_manager
        .load_session_metadata(workspace.path(), &session.session_id)
        .await
        .expect("metadata load")
        .expect("metadata should exist");
    assert!(metadata
        .custom_metadata
        .as_ref()
        .and_then(|value| value.get(EDIT_CONSTRAINT_METADATA_KEY))
        .is_some());

    manager.evict_loaded_session_for_test(&session.session_id);
    let restored_manager = test_manager(persistence_manager);
    restored_manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("session should restore");
    assert_eq!(
        restored_manager.edit_constraints(&session.session_id),
        Some(Vec::new())
    );
    let restored_state = restored_manager
        .edit_constraint_state(&session.session_id)
        .expect("constraint state should restore");
    assert_eq!(restored_state.extractions.len(), 2);
    assert_eq!(
        restored_state.extractions[1].revoked_constraint_ids,
        vec![constraint.id]
    );
    assert_eq!(
        restored_state.agent_created_paths,
        vec!["tests/temporary-repro.rs".to_string()]
    );
}
