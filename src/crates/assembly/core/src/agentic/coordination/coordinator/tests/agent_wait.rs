use super::super::test_support::{register_test_background_task, test_coordinator};
use super::super::BackgroundSubagentWaitMode;
use std::time::Duration;

#[tokio::test]
async fn background_subagent_outcome_is_consumed_only_by_agent_wait() {
    let (coordinator, _) = test_coordinator();
    let registered = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session",
    )
    .await;
    assert_eq!(registered.agent_id, "a1");
    assert_eq!(registered.bg_task_id, "a1_bg1");
    let completed = super::super::SubagentResult::completed("done".to_string());
    coordinator
        .background_subagent_outcomes
        .complete(registered.task_pk, Ok(&completed))
        .await;

    let result = coordinator
        .wait_for_background_subagent_outcomes(
            "parent-session",
            std::slice::from_ref(&registered.bg_task_id),
            BackgroundSubagentWaitMode::All,
            Duration::from_millis(10),
            "wait-turn-1",
            None,
            None,
        )
        .await
        .expect("AgentWait should collect the completed outcome");

    assert_eq!(result.status.as_str(), "completed");
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(result.outcomes[0].content.as_deref(), Some("done"));
    assert_eq!(result.outcomes[0].model_bg_task_id(), "a1_bg1");
    assert_eq!(result.outcomes[0].model_agent_id(), "a1");
    assert!(result.pending_bg_task_ids.is_empty());

    let second = coordinator
        .wait_for_background_subagent_outcomes(
            "parent-session",
            std::slice::from_ref(&registered.bg_task_id),
            BackgroundSubagentWaitMode::All,
            Duration::from_millis(10),
            "wait-turn-2",
            None,
            None,
        )
        .await
        .expect("a consumed outcome should not be delivered twice");
    assert_eq!(second.status.as_str(), "completed");
    assert!(second.outcomes.is_empty());
    assert_eq!(
        second.already_delivered_bg_task_ids,
        vec![registered.bg_task_id]
    );
}

#[tokio::test]
async fn agent_wait_returns_valid_results_and_missing_id_diagnostics_together() {
    let (coordinator, _) = test_coordinator();
    let registered = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session",
    )
    .await;
    let completed = super::super::SubagentResult::completed("done".to_string());
    coordinator
        .background_subagent_outcomes
        .complete(registered.task_pk, Ok(&completed))
        .await;

    let result = coordinator
        .wait_for_background_subagent_outcomes_with_selection(
            "parent-session",
            &[registered.bg_task_id.clone(), "bg-missing".to_string()],
            false,
            BackgroundSubagentWaitMode::All,
            Duration::from_millis(10),
            "wait-turn",
            None,
            None,
        )
        .await
        .expect("a missing ID should not prevent collecting valid results");

    assert_eq!(result.status.as_str(), "completed");
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(result.outcomes[0].bg_task_id, registered.bg_task_id);
    assert_eq!(result.missing_bg_task_ids, vec!["bg-missing"]);
    assert!(result.already_delivered_bg_task_ids.is_empty());
}

#[tokio::test]
async fn agent_wait_timeout_keeps_missing_id_diagnostics_with_pending_ids() {
    let (coordinator, _) = test_coordinator();
    let pending = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session",
    )
    .await;

    let result = coordinator
        .wait_for_background_subagent_outcomes_with_selection(
            "parent-session",
            &[pending.bg_task_id.clone(), "bg-missing".to_string()],
            false,
            BackgroundSubagentWaitMode::All,
            Duration::from_millis(1),
            "wait-turn",
            None,
            None,
        )
        .await
        .expect("a missing ID should not prevent waiting for valid pending IDs");

    assert_eq!(result.status.as_str(), "timed_out");
    assert_eq!(result.pending_bg_task_ids, vec![pending.bg_task_id]);
    assert_eq!(result.missing_bg_task_ids, vec!["bg-missing"]);
}

#[tokio::test]
async fn agent_wait_without_task_ids_collects_unconsumed_session_outcomes() {
    let (coordinator, _) = test_coordinator();
    let registered = register_test_background_task(
        &coordinator,
        "parent-session",
        "earlier-parent-turn",
        "subagent-session",
    )
    .await;
    let completed = super::super::SubagentResult::completed("done".to_string());
    coordinator
        .background_subagent_outcomes
        .complete(registered.task_pk, Ok(&completed))
        .await;

    let result = coordinator
        .wait_for_background_subagent_outcomes(
            "parent-session",
            &[],
            BackgroundSubagentWaitMode::Any,
            Duration::from_millis(10),
            "wait-turn",
            None,
            None,
        )
        .await
        .expect("AgentWait should collect a prior-turn outcome in the same session");

    assert_eq!(result.status.as_str(), "completed");
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(result.outcomes[0].bg_task_id, registered.bg_task_id);
    assert!(result.pending_bg_task_ids.is_empty());
}

#[tokio::test]
async fn agent_wait_all_times_out_with_returned_partial_results() {
    let (coordinator, _) = test_coordinator();
    let completed_task = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session-completed",
    )
    .await;
    let pending_task = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session-pending",
    )
    .await;
    let completed = super::super::SubagentResult::completed("done".to_string());
    coordinator
        .background_subagent_outcomes
        .complete(completed_task.task_pk, Ok(&completed))
        .await;

    let result = coordinator
        .wait_for_background_subagent_outcomes(
            "parent-session",
            &[],
            BackgroundSubagentWaitMode::All,
            Duration::from_millis(1),
            "wait-turn-1",
            None,
            None,
        )
        .await
        .expect("all selector timeout should return partial results");

    assert_eq!(result.status.as_str(), "timed_out");
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(result.outcomes[0].bg_task_id, completed_task.bg_task_id);
    assert_eq!(result.pending_bg_task_ids, vec![pending_task.bg_task_id]);

    let retry = coordinator
        .wait_for_background_subagent_outcomes(
            "parent-session",
            std::slice::from_ref(&completed_task.bg_task_id),
            BackgroundSubagentWaitMode::All,
            Duration::from_millis(10),
            "wait-turn-2",
            None,
            None,
        )
        .await
        .expect("returned results should be consumed");
    assert_eq!(retry.status.as_str(), "completed");
}

#[tokio::test]
async fn agent_wait_any_returns_partial_results_after_debounce() {
    let (coordinator, _) = test_coordinator();
    let completed_task = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session-completed",
    )
    .await;
    let pending_task = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session-pending",
    )
    .await;
    let completed = super::super::SubagentResult::completed("done".to_string());
    coordinator
        .background_subagent_outcomes
        .complete(completed_task.task_pk, Ok(&completed))
        .await;

    let result = coordinator
        .wait_for_background_subagent_outcomes(
            "parent-session",
            &[],
            BackgroundSubagentWaitMode::Any,
            Duration::from_secs(6),
            "wait-turn",
            None,
            None,
        )
        .await
        .expect("any selector should return after the result debounce");

    assert_eq!(result.status.as_str(), "completed");
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(result.outcomes[0].bg_task_id, completed_task.bg_task_id);
    assert_eq!(result.pending_bg_task_ids, vec![pending_task.bg_task_id]);
}

#[tokio::test]
async fn cancelled_agent_wait_keeps_collected_outcomes_available() {
    let (coordinator, _) = test_coordinator();
    let completed_task = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session-completed",
    )
    .await;
    let pending_task = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session-pending",
    )
    .await;
    let completed = super::super::SubagentResult::completed("done".to_string());
    coordinator
        .background_subagent_outcomes
        .complete(completed_task.task_pk, Ok(&completed))
        .await;

    let cancellation = tokio_util::sync::CancellationToken::new();
    cancellation.cancel();
    let requested_task_ids = vec![
        completed_task.bg_task_id.clone(),
        pending_task.bg_task_id.clone(),
    ];
    let error = coordinator
        .wait_for_background_subagent_outcomes(
            "parent-session",
            &requested_task_ids,
            BackgroundSubagentWaitMode::All,
            Duration::from_secs(10),
            "cancelled-wait-turn",
            Some(&cancellation),
            None,
        )
        .await
        .expect_err("cancelled AgentWait should not return a partial result");
    assert!(error.to_string().contains("AgentWait was cancelled"));

    let retry = coordinator
        .wait_for_background_subagent_outcomes(
            "parent-session",
            std::slice::from_ref(&completed_task.bg_task_id),
            BackgroundSubagentWaitMode::All,
            Duration::from_millis(10),
            "retry-wait-turn",
            None,
            None,
        )
        .await
        .expect("a cancelled wait must not consume the completed outcome");

    assert_eq!(retry.outcomes.len(), 1);
    assert_eq!(retry.outcomes[0].bg_task_id, completed_task.bg_task_id);
}

#[tokio::test]
async fn agent_wait_timeout_keeps_background_outcome_pending_without_follow_up() {
    let (coordinator, _) = test_coordinator();
    let registered = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session",
    )
    .await;

    let result = coordinator
        .wait_for_background_subagent_outcomes(
            "parent-session",
            std::slice::from_ref(&registered.bg_task_id),
            BackgroundSubagentWaitMode::All,
            Duration::from_millis(1),
            "wait-turn",
            None,
            None,
        )
        .await
        .expect("AgentWait timeout should be returned normally");

    assert_eq!(result.status.as_str(), "timed_out");
    assert!(result.outcomes.is_empty());
    assert_eq!(result.pending_bg_task_ids, vec![registered.bg_task_id]);
}

#[tokio::test]
async fn steered_agent_wait_returns_pending_tasks_without_cancelling_them() {
    let (coordinator, _) = test_coordinator();
    let registered = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session",
    )
    .await;
    let preemption = tokio_util::sync::CancellationToken::new();
    preemption.cancel();

    let result = coordinator
        .wait_for_background_subagent_outcomes(
            "parent-session",
            std::slice::from_ref(&registered.bg_task_id),
            BackgroundSubagentWaitMode::All,
            Duration::from_secs(10),
            "steered-wait-turn",
            None,
            Some(&preemption),
        )
        .await
        .expect("steering should end AgentWait normally");

    assert_eq!(result.status.as_str(), "steered");
    assert!(result.outcomes.is_empty());
    assert_eq!(result.pending_bg_task_ids, vec![registered.bg_task_id]);
}

#[tokio::test]
async fn steered_agent_wait_returns_and_consumes_available_partial_results() {
    let (coordinator, _) = test_coordinator();
    let completed_task = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session-completed",
    )
    .await;
    let pending_task = register_test_background_task(
        &coordinator,
        "parent-session",
        "parent-turn",
        "subagent-session-pending",
    )
    .await;
    let completed = super::super::SubagentResult::completed("done".to_string());
    coordinator
        .background_subagent_outcomes
        .complete(completed_task.task_pk, Ok(&completed))
        .await;
    let preemption = tokio_util::sync::CancellationToken::new();
    preemption.cancel();

    let result = coordinator
        .wait_for_background_subagent_outcomes(
            "parent-session",
            &[],
            BackgroundSubagentWaitMode::All,
            Duration::from_secs(10),
            "steered-wait-turn",
            None,
            Some(&preemption),
        )
        .await
        .expect("steering should return collected outcomes");

    assert_eq!(result.status.as_str(), "steered");
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(result.outcomes[0].bg_task_id, completed_task.bg_task_id);
    assert_eq!(result.pending_bg_task_ids, vec![pending_task.bg_task_id]);

    let retry = coordinator
        .wait_for_background_subagent_outcomes(
            "parent-session",
            std::slice::from_ref(&completed_task.bg_task_id),
            BackgroundSubagentWaitMode::All,
            Duration::from_millis(10),
            "retry-wait-turn",
            None,
            None,
        )
        .await
        .expect("a steered wait should consume returned outcomes");
    assert_eq!(retry.status.as_str(), "completed");
}
