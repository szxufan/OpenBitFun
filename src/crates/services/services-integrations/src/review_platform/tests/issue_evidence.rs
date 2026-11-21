use super::super::*;
use super::support::{github_issue_fixture, github_issue_identity};
use serde_json::json;

fn github_issue_comments_fixture() -> Value {
    json!([{
        "id": 7,
        "html_url": "https://github.com/example/repo/issues/42#issuecomment-7",
        "user": { "login": "bob" },
        "body": "The first comment.",
        "created_at": "2026-07-11T02:00:00Z",
        "updated_at": "2026-07-11T02:30:00Z"
    }])
}

#[test]
fn github_issue_mapping_preserves_body_labels_and_comment_pagination() {
    let evidence = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &github_issue_comments_fixture(),
        IssuePagination::new(Some(1), Some(100)),
        true,
        Some("2".to_string()),
    )
    .expect("GitHub fixture should map");

    assert_eq!(evidence.issue_id, "42");
    assert_eq!(evidence.body, "Keep the complete Issue body.");
    assert_eq!(evidence.labels, vec!["bug", "review"]);
    assert_eq!(evidence.comments.len(), 1);
    assert!(evidence.has_more_comments);
    assert_eq!(evidence.next_cursor.as_deref(), Some("2"));
    assert_eq!(evidence.completeness, ReviewEvidenceCompleteness::Partial);
    assert_eq!(
        evidence.limitations,
        vec!["issue_comments_paginated".to_string()]
    );
}

#[test]
fn gitlab_issue_mapping_preserves_description_labels_and_note_identity() {
    let identity = ProviderIssueIdentity::new(
        ReviewPlatformKind::Gitlab,
        "gitlab.com",
        "example/group/repo",
        "42",
    )
    .expect("GitLab identity fixture should be valid");
    let evidence = map_gitlab_issue(
        &identity,
        &json!({
            "iid": 42,
            "web_url": "https://gitlab.com/example/group/repo/-/issues/42",
            "title": "GitLab Issue evidence",
            "description": "Keep the complete GitLab description.",
            "state": "opened",
            "author": { "username": "alice" },
            "labels": ["review", "bug"],
            "created_at": "2026-07-11T00:00:00Z",
            "updated_at": "2026-07-11T01:00:00Z"
        }),
        &json!([{
            "id": 9,
            "author": { "username": "bob" },
            "body": "The first note.",
            "created_at": "2026-07-11T02:00:00Z",
            "updated_at": "2026-07-11T02:30:00Z",
            "system": false
        }]),
        IssuePagination::new(Some(1), Some(100)),
        false,
        None,
    )
    .expect("GitLab fixture should map");

    assert_eq!(evidence.issue_id, "42");
    assert_eq!(evidence.body, "Keep the complete GitLab description.");
    assert_eq!(evidence.labels, vec!["bug", "review"]);
    assert_eq!(evidence.comments[0].id, "9");
    assert_eq!(evidence.comments[0].author.as_deref(), Some("bob"));
    assert_eq!(evidence.completeness, ReviewEvidenceCompleteness::Complete);
    assert!(evidence.limitations.is_empty());
    assert!(!evidence.has_more_comments);
    assert!(evidence.next_cursor.is_none());
}

#[test]
fn issue_mapping_bounds_one_comment_page_to_one_hundred() {
    let comments = Value::Array(
        (0..101)
            .map(|id| {
                json!({
                    "id": id,
                    "user": { "login": "reviewer" },
                    "body": format!("comment {id}")
                })
            })
            .collect(),
    );
    let evidence = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &comments,
        IssuePagination::new(Some(1), Some(250)),
        false,
        None,
    )
    .expect("oversized fixture should be bounded");

    assert_eq!(evidence.comments.len(), 100);
    assert!(evidence.has_more_comments);
    assert_eq!(evidence.next_cursor.as_deref(), Some("2"));
    assert_eq!(evidence.completeness, ReviewEvidenceCompleteness::Partial);
    assert!(evidence
        .limitations
        .contains(&"issue_comments_page_bounded".to_string()));
}

#[test]
fn issue_mapping_later_page_reports_omitted_previous_comments() {
    let evidence = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &github_issue_comments_fixture(),
        IssuePagination::new(Some(2), Some(100)),
        false,
        None,
    )
    .expect("later page fixture should map");

    assert_eq!(evidence.completeness, ReviewEvidenceCompleteness::Partial);
    assert!(evidence
        .limitations
        .contains(&"issue_comments_previous_pages_omitted".to_string()));
    assert!(!evidence.has_more_comments);
    assert!(evidence.next_cursor.is_none());
}

#[test]
fn issue_mapping_rejects_github_pull_request_payloads_with_typed_error() {
    let mut issue = github_issue_fixture();
    issue["pull_request"] = json!({
        "url": "https://api.github.com/repos/example/repo/pulls/42"
    });

    let result = map_github_issue(
        &github_issue_identity(),
        &issue,
        &github_issue_comments_fixture(),
        IssuePagination::new(None, None),
        false,
        None,
    );

    assert!(matches!(
        result,
        Err(ReviewPlatformError::TargetIsPullRequest { .. })
    ));
}

#[test]
fn issue_mapping_rejects_issue_body_over_unicode_scalar_budget() {
    let mut issue = github_issue_fixture();
    issue["body"] = json!("界".repeat(MAX_ISSUE_BODY_CHARS + 1));

    let result = map_github_issue(
        &github_issue_identity(),
        &issue,
        &json!([]),
        IssuePagination::new(None, None),
        false,
        None,
    );

    assert!(matches!(
        result,
        Err(ReviewPlatformError::EvidenceTooLarge { ref resource, .. })
            if resource == "issue_body"
    ));
}

#[test]
fn issue_mapping_truncates_oversized_comment_bodies_as_partial_evidence() {
    let comments = json!([{
        "id": 7,
        "user": { "login": "bob" },
        "body": "界".repeat(MAX_ISSUE_COMMENT_BODY_CHARS + 1),
        "created_at": "2026-07-11T02:00:00Z"
    }]);

    let evidence = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &comments,
        IssuePagination::new(None, None),
        false,
        None,
    )
    .expect("oversized comment should degrade structurally");

    assert_eq!(
        evidence.comments[0].body.chars().count(),
        MAX_ISSUE_COMMENT_BODY_CHARS
    );
    assert_eq!(evidence.completeness, ReviewEvidenceCompleteness::Partial);
    assert!(evidence
        .limitations
        .contains(&"issue_comment_body_truncated".to_string()));
}

#[test]
fn issue_mapping_bounds_aggregate_comment_unicode_scalars() {
    let comments = Value::Array(
        (1..=17)
            .map(|id| {
                json!({
                    "id": id,
                    "user": { "login": "reviewer" },
                    "body": "界".repeat(MAX_ISSUE_COMMENT_BODY_CHARS),
                    "created_at": format!("2026-07-11T{id:02}:00:00Z")
                })
            })
            .collect(),
    );

    let evidence = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &comments,
        IssuePagination::new(None, None),
        false,
        None,
    )
    .expect("aggregate overflow should degrade structurally");
    let aggregate_chars = evidence
        .comments
        .iter()
        .map(|comment| comment.body.chars().count())
        .sum::<usize>();

    assert!(aggregate_chars <= MAX_ISSUE_COMMENTS_AGGREGATE_CHARS);
    assert_eq!(evidence.completeness, ReviewEvidenceCompleteness::Partial);
    assert!(evidence
        .limitations
        .contains(&"issue_comments_aggregate_truncated".to_string()));
}

#[test]
fn gitlab_issue_mapping_filters_system_notes_and_orders_comments_ascending() {
    let identity = ProviderIssueIdentity::new(
        ReviewPlatformKind::Gitlab,
        "gitlab.com",
        "example/repo",
        "42",
    )
    .expect("GitLab identity should be valid");
    let issue = json!({
        "iid": 42,
        "title": "Issue",
        "description": "Body",
        "state": "opened"
    });
    let notes = json!([
        { "id": 2, "body": "later", "created_at": "2026-07-11T03:00:00Z", "system": false },
        { "id": 99, "body": "system", "created_at": "2026-07-11T02:30:00Z", "system": true },
        { "id": 1, "body": "earlier", "created_at": "2026-07-11T02:00:00Z", "system": false }
    ]);

    let evidence = map_gitlab_issue(
        &identity,
        &issue,
        &notes,
        IssuePagination::new(None, None),
        false,
        None,
    )
    .expect("GitLab notes should map");

    assert_eq!(
        evidence
            .comments
            .iter()
            .map(|comment| comment.id.as_str())
            .collect::<Vec<_>>(),
        vec!["1", "2"]
    );
}

#[test]
fn issue_mapping_fingerprint_is_deterministic_and_content_sensitive() {
    let first = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &github_issue_comments_fixture(),
        IssuePagination::new(None, None),
        false,
        None,
    )
    .expect("first fixture should map");
    let reordered = map_github_issue(
        &github_issue_identity(),
        &json!({
            "updated_at": "2026-07-11T01:00:00Z",
            "labels": [{ "name": "bug" }, { "name": "review" }],
            "state": "open",
            "title": "Provider Issue evidence",
            "number": 42,
            "body": "Keep the complete Issue body.",
            "html_url": "https://github.com/example/repo/issues/42",
            "created_at": "2026-07-11T00:00:00Z",
            "user": { "login": "alice" }
        }),
        &github_issue_comments_fixture(),
        IssuePagination::new(None, None),
        false,
        None,
    )
    .expect("reordered fixture should map");
    let changed = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &json!([{
            "id": 7,
            "html_url": "https://github.com/example/repo/issues/42#issuecomment-7",
            "user": { "login": "bob" },
            "body": "Changed comment content.",
            "created_at": "2026-07-11T02:00:00Z",
            "updated_at": "2026-07-11T02:30:00Z"
        }]),
        IssuePagination::new(None, None),
        false,
        None,
    )
    .expect("changed fixture should map");

    assert_eq!(first.fingerprint, reordered.fingerprint);
    assert_ne!(first.fingerprint, changed.fingerprint);
    assert!(first.fingerprint.starts_with("sha256:"));
}

#[test]
fn issue_mapping_fingerprint_canonicalizes_comment_order() {
    let comments = json!([
        {
            "id": 8,
            "user": { "login": "carol" },
            "body": "Later comment.",
            "created_at": "2026-07-11T03:00:00Z"
        },
        {
            "id": 7,
            "user": { "login": "bob" },
            "body": "Earlier comment.",
            "created_at": "2026-07-11T02:00:00Z"
        }
    ]);
    let reversed = Value::Array(
        comments
            .as_array()
            .expect("fixture should be an array")
            .iter()
            .rev()
            .cloned()
            .collect(),
    );
    let first = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &comments,
        IssuePagination::new(None, None),
        false,
        None,
    )
    .expect("first order should map");
    let second = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &reversed,
        IssuePagination::new(None, None),
        false,
        None,
    )
    .expect("reversed order should map");

    assert_eq!(first.fingerprint, second.fingerprint);
    assert_eq!(first.comments[0].id, "7");
    assert_eq!(second.comments[0].id, "7");
}

#[test]
fn issue_mapping_fingerprint_includes_page_and_evidence_state() {
    let complete = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &github_issue_comments_fixture(),
        IssuePagination::new(Some(1), Some(100)),
        false,
        None,
    )
    .expect("complete page should map");
    let has_more = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &github_issue_comments_fixture(),
        IssuePagination::new(Some(1), Some(100)),
        true,
        Some("2".to_string()),
    )
    .expect("partial page should map");
    let later_page = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &github_issue_comments_fixture(),
        IssuePagination::new(Some(2), Some(100)),
        false,
        None,
    )
    .expect("later page should map");

    assert_ne!(complete.fingerprint, has_more.fingerprint);
    assert_ne!(complete.fingerprint, later_page.fingerprint);
    assert_ne!(has_more.fingerprint, later_page.fingerprint);
}

#[test]
fn issue_mapping_rejects_non_array_comment_payloads() {
    let result = map_github_issue(
        &github_issue_identity(),
        &github_issue_fixture(),
        &json!({ "message": "not an array" }),
        IssuePagination::new(None, None),
        false,
        None,
    );

    assert!(matches!(result, Err(ReviewPlatformError::Parse(_))));
}
