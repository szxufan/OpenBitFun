use super::super::test_support::{
    agent_lifecycle_child, register_test_background_task, test_coordinator,
    test_persistent_coordinator,
};
use super::super::{normalize_subagent_max_concurrency, SubagentExecutionRequest};
use super::super::{BackgroundSubagentCancelTarget, SubagentResult, SubagentResultStatus};
use crate::agentic::core::{
    SessionConfig, SessionContinuationPolicy, SessionModelBindingPolicy, SessionState,
};
use crate::agentic::tools::pipeline::SubagentParentInfo;
use crate::util::errors::OpenBitFunError;
use openbitfun_runtime_ports::{DelegationPolicy, PermissionRuntimeCeiling, SubagentContextMode};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn subagent_concurrency_uses_global_capacity_for_all_ordinary_agent_types() {
    use super::super::SubagentConcurrencyPermitGuard;
    use tokio::time::Instant;

    let (coordinator, _) = test_coordinator();
    let limiter = coordinator.get_subagent_concurrency_limiter().await;
    let capacity = limiter.max_concurrency;
    let agent_types = [
        "Explore",
        "Cowork",
        "ResearchSpecialist",
        "ReviewSecurity",
        "Standard",
    ];
    let mut guards = Vec::new();
    for index in 0..capacity {
        let agent_type = agent_types[index % agent_types.len()];
        let (permits, _) = coordinator
            .acquire_subagent_concurrency_permit(
                agent_type,
                None,
                Some(Instant::now() + Duration::from_secs(5)),
            )
            .await
            .expect("ordinary agents should share all configured global slots");
        assert_eq!(permits.len(), 1);
        assert!(Arc::ptr_eq(&permits[0].1.semaphore, &limiter.semaphore));
        guards.push(SubagentConcurrencyPermitGuard::new(
            permits,
            agent_type.to_string(),
        ));
    }
    assert_eq!(limiter.semaphore.available_permits(), 0);

    let timed_out = coordinator
        .acquire_subagent_concurrency_permit("Explore", None, Some(Instant::now()))
        .await;
    assert!(matches!(
        timed_out,
        Err(crate::util::errors::OpenBitFunError::Timeout(_))
    ));

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let cancelled_wait = coordinator
        .acquire_subagent_concurrency_permit("ResearchSpecialist", Some(&cancelled), None)
        .await;
    assert!(matches!(
        cancelled_wait,
        Err(crate::util::errors::OpenBitFunError::Cancelled(_))
    ));

    guards.pop();
    let (permits, _) = coordinator
        .acquire_subagent_concurrency_permit(
            "ResearchSpecialist",
            None,
            Some(Instant::now() + Duration::from_secs(5)),
        )
        .await
        .expect("releasing a slot should admit a different ordinary agent type");
    drop(SubagentConcurrencyPermitGuard::new(
        permits,
        "ResearchSpecialist".to_string(),
    ));
    drop(guards);
    assert_eq!(limiter.semaphore.available_permits(), capacity);
}

#[tokio::test]
async fn background_subagent_start_honors_an_already_cancelled_tool() {
    let (coordinator, _session_manager) = test_coordinator();
    let cancellation_token = CancellationToken::new();
    cancellation_token.cancel();
    let request = SubagentExecutionRequest {
        task_description: "should not start".to_string(),
        requested_agent_id: None,
        context_mode: SubagentContextMode::Fresh,
        target_session_id: None,
        subagent_type: Some("Explore".to_string()),
        logical_subagent_type: Some("Explore".to_string()),
        continuation_policy: SessionContinuationPolicy::Reusable,
        model_binding_policy: SessionModelBindingPolicy::Mutable,
        workspace_path: None,
        model_id: None,
        inherit_parent_model: false,
        subagent_parent_info: SubagentParentInfo {
            session_id: "parent-session".to_string(),
            dialog_turn_id: "parent-turn".to_string(),
            tool_call_id: "task-tool".to_string(),
        },
        context: HashMap::new(),
        permission_runtime_ceiling: PermissionRuntimeCeiling::default(),
        delegation_policy: DelegationPolicy::top_level().spawn_child(),
        external_generation_lease: None,
    };

    let error = coordinator
        .start_background_subagent(request, None, Some(cancellation_token))
        .await
        .expect_err("a cancelled Tool must not start a background subagent");

    assert!(matches!(
        error,
        crate::util::errors::OpenBitFunError::Cancelled(_)
    ));
}

#[tokio::test]
async fn agent_lifecycle_fresh_only_and_failed_children_can_be_cancelled() {
    let workspace = tempfile::tempdir().unwrap();
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, manager) = test_coordinator();
    for policy in [
        SessionContinuationPolicy::FreshOnly,
        SessionContinuationPolicy::Reusable,
    ] {
        let child = agent_lifecycle_child(&coordinator, workspace.path(), "parent", policy).await;
        if policy == SessionContinuationPolicy::Reusable {
            manager
                .update_session_state(
                    &child.session_id,
                    SessionState::Error {
                        error: "failed execution".into(),
                        recoverable: false,
                    },
                )
                .await
                .unwrap();
        }
        assert!(coordinator
            .ensure_subagent_session_loaded_for_reuse(&child.session_id, "parent")
            .await
            .is_err());
        assert!(coordinator
            .cancel_background_subagents_for_parent("other-parent", &child.session_id, true)
            .await
            .is_err());
        coordinator
            .cancel_background_subagents_for_parent("parent", &child.session_id, true)
            .await
            .expect("lifecycle operations must not require continuation eligibility");
    }
}

#[tokio::test]
async fn agent_lifecycle_interrupt_cancels_direct_user_turn() {
    let workspace = tempfile::tempdir().unwrap();
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, manager) = test_coordinator();
    let child = agent_lifecycle_child(
        &coordinator,
        workspace.path(),
        "parent",
        SessionContinuationPolicy::Reusable,
    )
    .await;
    let turn = manager
        .start_dialog_turn(
            &child.session_id,
            "GeneralPurpose".into(),
            "Follow up".into(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let token = CancellationToken::new();
    coordinator
        .execution_engine
        .register_cancel_token(&turn, token.clone());
    let (cancelled, ()) = tokio::join!(
        coordinator.cancel_background_subagents_for_parent("parent", &child.session_id, true),
        async {
            token.cancelled().await;
            coordinator
                .execution_engine
                .cleanup_cancel_token(&turn)
                .await;
        }
    );
    cancelled.unwrap();
    assert!(token.is_cancelled());
    assert!(matches!(
        manager.get_session(&child.session_id).unwrap().state,
        SessionState::Idle
    ));
}

#[tokio::test]
async fn agent_lifecycle_interrupt_crosses_inactive_ordinary_ancestor() {
    let workspace = tempfile::tempdir().unwrap();
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, _) = test_coordinator();
    let child = agent_lifecycle_child(
        &coordinator,
        workspace.path(),
        "parent",
        SessionContinuationPolicy::Reusable,
    )
    .await;
    let middle = agent_lifecycle_child(
        &coordinator,
        workspace.path(),
        &child.session_id,
        SessionContinuationPolicy::Reusable,
    )
    .await;
    let leaf = agent_lifecycle_child(
        &coordinator,
        workspace.path(),
        &middle.session_id,
        SessionContinuationPolicy::Reusable,
    )
    .await;
    let task = register_test_background_task(
        &coordinator,
        &middle.session_id,
        "turn-middle",
        &leaf.session_id,
    )
    .await;
    let token = CancellationToken::new();
    coordinator.register_background_subagent_task(
        task.task_pk,
        middle.session_id.clone(),
        leaf.session_id.clone(),
        BackgroundSubagentCancelTarget::Direct(token.clone()),
    );
    assert_eq!(
        coordinator
            .cancel_background_subagents_for_parent("parent", &child.session_id, false)
            .await
            .unwrap(),
        0
    );
    assert!(!token.is_cancelled());
    assert_eq!(
        coordinator
            .cancel_background_subagents_for_parent("parent", &child.session_id, true)
            .await
            .unwrap(),
        1
    );
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn agent_lifecycle_list_tracks_foreground_turns_after_background_completion() {
    let workspace = tempfile::tempdir().unwrap();
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, manager) = test_coordinator();
    let child = agent_lifecycle_child(
        &coordinator,
        workspace.path(),
        "parent",
        SessionContinuationPolicy::Reusable,
    )
    .await;
    let turn = manager
        .start_dialog_turn(
            &child.session_id,
            "GeneralPurpose".into(),
            "Initial task".into(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    manager
        .complete_dialog_turn(
            &child.session_id,
            &turn,
            "Done".into(),
            &[],
            Default::default(),
            Some("complete".into()),
            Some(true),
        )
        .await
        .unwrap();
    manager
        .update_session_state(&child.session_id, SessionState::Idle)
        .await
        .unwrap();
    assert_eq!(
        coordinator.direct_child_agents("parent").await.unwrap()[0]
            .status
            .as_str(),
        "completed"
    );
    let task =
        register_test_background_task(&coordinator, "parent", "parent-turn", &child.session_id)
            .await;
    coordinator
        .background_subagent_outcomes
        .complete(
            task.task_pk,
            Err(&OpenBitFunError::Cancelled("old task".into())),
        )
        .await;
    let turn = manager
        .start_dialog_turn(
            &child.session_id,
            "GeneralPurpose".into(),
            "Follow up".into(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        coordinator.direct_child_agents("parent").await.unwrap()[0]
            .status
            .as_str(),
        "running"
    );
    manager
        .complete_dialog_turn(
            &child.session_id,
            &turn,
            "Done again".into(),
            &[],
            Default::default(),
            Some("complete".into()),
            Some(true),
        )
        .await
        .unwrap();
    manager
        .update_session_state(&child.session_id, SessionState::Idle)
        .await
        .unwrap();
    assert_eq!(
        coordinator.direct_child_agents("parent").await.unwrap()[0]
            .status
            .as_str(),
        "completed"
    );
    for expected in ["failed", "cancelled"] {
        let turn = manager
            .start_dialog_turn(
                &child.session_id,
                "GeneralPurpose".into(),
                "Another follow up".into(),
                None,
                None,
                None,
            )
            .await
            .unwrap();
        if expected == "failed" {
            manager
                .fail_dialog_turn(&child.session_id, &turn, "Test failure".into())
                .await
                .unwrap();
        } else {
            manager
                .cancel_dialog_turn(&child.session_id, &turn)
                .await
                .unwrap();
        }
        manager
            .update_session_state(&child.session_id, SessionState::Idle)
            .await
            .unwrap();
        assert_eq!(
            coordinator.direct_child_agents("parent").await.unwrap()[0]
                .status
                .as_str(),
            expected
        );
    }
}

#[tokio::test]
async fn agent_lifecycle_cold_fresh_only_and_failed_subtree_can_be_deleted() {
    let workspace = tempfile::tempdir().unwrap();
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, manager) = test_persistent_coordinator();
    let parent = manager
        .create_session(
            "Parent".into(),
            "Standard".into(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let child = agent_lifecycle_child(
        &coordinator,
        workspace.path(),
        &parent.session_id,
        SessionContinuationPolicy::FreshOnly,
    )
    .await;
    let leaf = agent_lifecycle_child(
        &coordinator,
        workspace.path(),
        &child.session_id,
        SessionContinuationPolicy::Reusable,
    )
    .await;
    manager
        .update_session_state(
            &leaf.session_id,
            SessionState::Error {
                error: "Test failure".into(),
                recoverable: false,
            },
        )
        .await
        .unwrap();
    let id = coordinator
        .existing_agent_id_for_subagent_session(&parent.session_id, &child.session_id)
        .await
        .unwrap()
        .unwrap();
    manager
        .unload_session_from_memory(&child.session_id)
        .await
        .unwrap();
    manager
        .unload_session_from_memory(&leaf.session_id)
        .await
        .unwrap();
    assert_eq!(
        coordinator
            .delete_direct_child_agents(&parent.session_id, &[id])
            .await
            .unwrap(),
        2
    );
    assert!(manager.get_session(&child.session_id).is_none());
    assert!(manager.get_session(&leaf.session_id).is_none());
    assert!(manager.get_session(&parent.session_id).is_some());
    assert!(coordinator
        .direct_child_agents(&parent.session_id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn agent_lifecycle_list_uses_cold_summary_and_preserves_queued_and_partial_outcomes() {
    let workspace = tempfile::tempdir().unwrap();
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, manager) = test_persistent_coordinator();
    let parent = manager
        .create_session(
            "Parent".into(),
            "Standard".into(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let child = agent_lifecycle_child(
        &coordinator,
        workspace.path(),
        &parent.session_id,
        SessionContinuationPolicy::Reusable,
    )
    .await;
    let turn = manager
        .start_dialog_turn(
            &child.session_id,
            "GeneralPurpose".into(),
            "Task".into(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    manager
        .complete_dialog_turn(
            &child.session_id,
            &turn,
            "Done".into(),
            &[],
            Default::default(),
            Some("complete".into()),
            Some(true),
        )
        .await
        .unwrap();
    manager
        .update_session_state(&child.session_id, SessionState::Idle)
        .await
        .unwrap();
    manager
        .unload_session_from_memory(&child.session_id)
        .await
        .unwrap();
    assert_eq!(
        coordinator
            .direct_child_agents(&parent.session_id)
            .await
            .unwrap()[0]
            .status
            .as_str(),
        "completed"
    );
    assert!(
        manager.get_session(&child.session_id).is_none(),
        "listing must not restore a runtime"
    );
    let storage = manager
        .require_session_storage_path(&parent.session_id)
        .await
        .unwrap();
    manager
        .update_session_metadata(&storage, &child.session_id, |metadata| {
            metadata.last_turn = None
        })
        .await
        .unwrap();
    assert_eq!(
        coordinator
            .direct_child_agents(&parent.session_id)
            .await
            .unwrap()[0]
            .status
            .as_str(),
        "completed",
        "legacy foreground registration is terminal"
    );
    let task = register_test_background_task(
        &coordinator,
        &parent.session_id,
        "parent-turn",
        &child.session_id,
    )
    .await;
    assert_eq!(
        coordinator
            .direct_child_agents(&parent.session_id)
            .await
            .unwrap()[0]
            .status
            .as_str(),
        "running",
        "queued task must override old completion"
    );
    coordinator
        .load_owned_subagent_session(&child.session_id, &parent.session_id)
        .await
        .unwrap();
    let turn = format!("turn-{}", child.session_id);
    manager
        .start_dialog_turn(
            &child.session_id,
            "GeneralPurpose".into(),
            "Background work".into(),
            Some(turn.clone()),
            None,
            None,
        )
        .await
        .unwrap();
    manager
        .cancel_dialog_turn(&child.session_id, &turn)
        .await
        .unwrap();
    manager
        .update_session_state(&child.session_id, SessionState::Idle)
        .await
        .unwrap();
    coordinator
        .background_subagent_outcomes
        .complete(
            task.task_pk,
            Ok(&SubagentResult {
                text: "Partial result".into(),
                status: SubagentResultStatus::PartialTimeout,
                reason: None,
                ledger_event_id: None,
                session_id: Some(child.session_id.clone()),
            }),
        )
        .await;
    manager
        .unload_session_from_memory(&child.session_id)
        .await
        .unwrap();
    assert_eq!(
        coordinator
            .direct_child_agents(&parent.session_id)
            .await
            .unwrap()[0]
            .status
            .as_str(),
        "partial_timeout"
    );
}

#[test]
fn clamps_subagent_max_concurrency_into_safe_range() {
    assert_eq!(normalize_subagent_max_concurrency(0), 1);
    assert_eq!(normalize_subagent_max_concurrency(5), 5);
    assert_eq!(normalize_subagent_max_concurrency(usize::MAX), 64);
}

#[test]
fn subagent_timeout_disable_clears_active_deadline() {
    use super::super::SubagentTimeoutAction;
    use std::sync::Mutex;
    use tokio::sync::watch;
    use tokio::time::{Duration, Instant};

    let initial_deadline = Instant::now() + Duration::from_secs(1200);
    let (deadline_tx, mut deadline_rx) = watch::channel(Some(initial_deadline));
    let handle = super::super::SubagentTimeoutHandle {
        deadline_tx,
        session_id: "subagent-session".to_string(),
        original_timeout_seconds: Some(1200),
        remaining_at_pause: Mutex::new(None),
    };

    handle.apply_action(SubagentTimeoutAction::Disable);

    assert!(deadline_rx.borrow_and_update().is_none());
}
