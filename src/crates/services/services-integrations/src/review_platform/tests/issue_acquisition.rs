use super::super::*;
use super::support::{github_issue_fixture, github_issue_identity};
use serde_json::json;

#[test]
fn issue_acquisition_request_plans_use_exact_github_and_gitlab_endpoints() {
    assert_eq!(MAX_ISSUE_RESPONSE_BYTES, 2 * 1024 * 1024);
    assert_eq!(MAX_ISSUE_COMMENTS_RESPONSE_BYTES, 8 * 1024 * 1024);
    let tokens = ReviewPlatformAuthTokens::default();
    let github_identity = github_issue_identity();
    let github_context = provider_context_for_identity(
        github_identity.platform,
        &github_identity.host,
        &github_identity.project_path,
        &tokens,
    )
    .expect("GitHub context should be valid");
    let github = issue_request_plan(
        &github_context,
        &github_identity,
        IssuePagination::new(Some(2), Some(250)),
    )
    .expect("GitHub request plan should be valid");
    assert_eq!(
        github.issue_url,
        "https://api.github.com/repos/example/repo/issues/42"
    );
    assert_eq!(
        github.comments_url,
        "https://api.github.com/repos/example/repo/issues/42/comments"
    );
    assert_eq!(github.pagination.page, 2);
    assert_eq!(github.pagination.per_page, 100);

    let gitlab_identity = ProviderIssueIdentity::new(
        ReviewPlatformKind::Gitlab,
        "gitlab.com",
        "example/group/repo",
        "42",
    )
    .expect("GitLab identity should be valid");
    let gitlab_context = provider_context_for_identity(
        gitlab_identity.platform,
        &gitlab_identity.host,
        &gitlab_identity.project_path,
        &tokens,
    )
    .expect("GitLab context should be valid");
    let gitlab = issue_request_plan(
        &gitlab_context,
        &gitlab_identity,
        IssuePagination::new(None, None),
    )
    .expect("GitLab request plan should be valid");
    assert_eq!(
        gitlab.issue_url,
        "https://gitlab.com/api/v4/projects/example%2Fgroup%2Frepo/issues/42"
    );
    assert_eq!(
        gitlab.comments_url,
        "https://gitlab.com/api/v4/projects/example%2Fgroup%2Frepo/issues/42/notes"
    );
    assert_eq!(
        gitlab.comments_query,
        vec![
            ("order_by".to_string(), "created_at".to_string()),
            ("sort".to_string(), "asc".to_string()),
            ("activity_filter".to_string(), "only_comments".to_string()),
        ]
    );
}

#[test]
fn issue_acquisition_maps_oversized_detail_to_typed_unavailable_error() {
    let error = review_evidence_http_error(
        ReviewHttpError::ResponseTooLarge {
            limit_bytes: MAX_ISSUE_RESPONSE_BYTES,
        },
        "issue_response",
    );

    assert!(matches!(
        error,
        ReviewPlatformError::EvidenceTooLarge { ref resource, limit }
            if resource == "issue_response" && limit == MAX_ISSUE_RESPONSE_BYTES
    ));
}

#[test]
fn issue_acquisition_maps_oversized_comments_to_structured_partial_evidence() {
    let evidence = map_issue_comments_response(
        &github_issue_identity(),
        &github_issue_fixture(),
        IssuePagination::new(None, None),
        Err(ReviewPlatformError::EvidenceTooLarge {
            resource: "issue_comments_response".to_string(),
            limit: MAX_ISSUE_COMMENTS_RESPONSE_BYTES,
        }),
    )
    .expect("oversized comments should degrade structurally");

    assert!(evidence.comments.is_empty());
    assert_eq!(evidence.completeness, ReviewEvidenceCompleteness::Partial);
    assert!(evidence
        .limitations
        .contains(&"issue_comments_response_too_large".to_string()));
    assert!(!evidence.has_more_comments);
    assert!(evidence.next_cursor.is_none());
}

#[test]
fn issue_acquisition_rejects_github_pull_request_payload() {
    let identity = github_issue_identity();
    let result = reject_pull_request_issue_target(
        &identity,
        &json!({
            "number": 42,
            "pull_request": {
                "url": "https://api.github.com/repos/example/repo/pulls/42"
            }
        }),
    );

    assert!(matches!(
        result,
        Err(ReviewPlatformError::TargetIsPullRequest { .. })
    ));
}
