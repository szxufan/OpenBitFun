use super::super::delegation_policy_for_agent_turn;

#[test]
fn agent_turn_delegation_policy_recovers_swarm_scope_and_depth() {
    let ultra = delegation_policy_for_agent_turn("Ultimate", None, true)
        .expect("Ultimate should start a Swarm tree");
    assert!(ultra.allow_subagent_spawn);
    assert_eq!(ultra.nesting_depth, 0);
    assert_eq!(
        ultra.scope,
        openbitfun_runtime_ports::DelegationScope::Swarm
    );

    let planner = delegation_policy_for_agent_turn("SwarmPlanner", Some(2), true)
        .expect("a persisted planner should recover its tree depth");
    assert!(planner.allow_subagent_spawn);
    assert_eq!(planner.nesting_depth, 2);
    assert_eq!(
        planner.scope,
        openbitfun_runtime_ports::DelegationScope::Swarm
    );

    let worker = delegation_policy_for_agent_turn("SwarmWorker", Some(2), false)
        .expect("workers use the standard non-recursive turn policy");
    assert!(!worker.allow_subagent_spawn);
    assert_eq!(worker.nesting_depth, 2);
    assert_eq!(
        worker.scope,
        openbitfun_runtime_ports::DelegationScope::Swarm
    );
}

#[test]
fn swarm_planner_turn_fails_closed_without_persisted_depth() {
    let error = delegation_policy_for_agent_turn("SwarmPlanner", None, true)
        .expect_err("a planner without a persisted tree node must not execute");
    assert!(error
        .to_string()
        .contains("missing its persisted tree node"));
}

#[test]
fn every_swarm_delegate_turn_fails_closed_without_persisted_depth() {
    for agent_type in ["SwarmPlanner", "SwarmWorker", "SwarmReviewer"] {
        let error = delegation_policy_for_agent_turn(agent_type, None, true)
            .expect_err("a Swarm delegate without a persisted tree node must not execute");
        assert!(error
            .to_string()
            .contains("missing its persisted tree node"));
    }
}

#[test]
fn subagent_lineage_ownership_requires_matching_parent() {
    use crate::service::session::{SessionRelationship, SessionRelationshipKind};

    let relationship = SessionRelationship {
        kind: Some(SessionRelationshipKind::Subagent),
        parent_session_id: Some("parent-session".to_string()),
        parent_request_id: None,
        parent_dialog_turn_id: None,
        parent_turn_index: None,
        parent_tool_call_id: None,
        subagent_type: None,
        continuation_policy: None,
    };

    assert!(super::super::session_lineage_matches_parent(
        Some(&relationship),
        "parent-session"
    ));
    assert!(!super::super::session_lineage_matches_parent(
        Some(&relationship),
        "other-parent"
    ));
    assert!(!super::super::session_lineage_matches_parent(
        None,
        "parent-session"
    ));
}

#[test]
fn persisted_subagent_lineage_restores_permission_delegation_context() {
    use crate::service::session::{SessionRelationship, SessionRelationshipKind};

    let relationship = SessionRelationship {
        kind: Some(SessionRelationshipKind::Subagent),
        parent_session_id: Some("parent-session".to_string()),
        parent_request_id: None,
        parent_dialog_turn_id: Some("parent-turn".to_string()),
        parent_turn_index: None,
        parent_tool_call_id: Some("task-tool-call".to_string()),
        subagent_type: Some("Explore".to_string()),
        continuation_policy: None,
    };

    assert_eq!(
        super::super::subagent_parent_info_from_relationship(Some(&relationship)).map(|info| (
            info.session_id,
            info.dialog_turn_id,
            info.tool_call_id
        )),
        Some((
            "parent-session".to_string(),
            "parent-turn".to_string(),
            "task-tool-call".to_string(),
        ))
    );
}

#[test]
fn persisted_subagent_lineage_without_parent_turn_preserves_permission_routing() {
    use crate::service::session::{SessionRelationship, SessionRelationshipKind};

    let relationship = SessionRelationship {
        kind: Some(SessionRelationshipKind::Subagent),
        parent_session_id: Some("parent-session".to_string()),
        parent_request_id: None,
        parent_dialog_turn_id: None,
        parent_turn_index: None,
        parent_tool_call_id: Some("task-tool-call".to_string()),
        subagent_type: Some("Explore".to_string()),
        continuation_policy: None,
    };

    assert!(super::super::subagent_parent_info_from_relationship(Some(&relationship)).is_none());
    assert_eq!(
        super::super::permission_delegation_from_relationship(
            Some(&relationship),
            "GeneralPurpose"
        )
        .map(|delegation| (
            delegation.parent_session_id,
            delegation.parent_dialog_turn_id,
            delegation.parent_tool_call_id,
            delegation.subagent_type,
        )),
        Some((
            "parent-session".to_string(),
            None,
            "task-tool-call".to_string(),
            "Explore".to_string(),
        ))
    );
}
