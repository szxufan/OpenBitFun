use super::super::*;
use super::support::temp_token_store_path;
use serde_json::json;

#[test]
fn review_target_reports_unavailable_provider_diffs_without_embedding_them() {
    let mut pull_request = github_pull_request_from_value(&json!({
        "number": 42,
        "title": "Review target",
        "state": "open",
        "head": { "ref": "feature", "sha": "2222222222222222222222222222222222222222" },
        "base": { "ref": "main", "sha": "1111111111111111111111111111111111111111" },
        "changed_files": 2
    }));
    pull_request.changed_files = 2;
    let target = review_target_from_parts(
        pull_request,
        vec![
            ReviewPlatformFile {
                path: "src/lib.rs".to_string(),
                old_path: None,
                status: ReviewFileStatus::Modified,
                additions: 1,
                deletions: 1,
                patch: Some("@@ -1 +1 @@\n-old\n+new".to_string()),
            },
            ReviewPlatformFile {
                path: "assets/image.png".to_string(),
                old_path: None,
                status: ReviewFileStatus::Modified,
                additions: 0,
                deletions: 0,
                patch: None,
            },
        ],
    );

    assert_eq!(target.files.len(), 2);
    assert!(target.files[0].diff_available);
    assert!(!target.files[1].diff_available);
    assert!(target
        .limitations
        .contains(&"provider_file_diff_unavailable".to_string()));
}

#[test]
fn review_target_keeps_more_than_five_hundred_provider_files() {
    let mut pull_request = github_pull_request_from_value(&json!({
        "number": 42,
        "title": "Large review target",
        "state": "open",
        "head": { "ref": "feature", "sha": "2222222222222222222222222222222222222222" },
        "base": { "ref": "main", "sha": "1111111111111111111111111111111111111111" },
        "changed_files": 501
    }));
    pull_request.changed_files = 501;
    let files = (0..501)
        .map(|index| ReviewPlatformFile {
            path: format!("src/file-{index}.rs"),
            old_path: None,
            status: ReviewFileStatus::Modified,
            additions: 1,
            deletions: 1,
            patch: Some("@@ -1 +1 @@\n-old\n+new".to_string()),
        })
        .collect();

    let target = review_target_from_parts(pull_request, files);

    assert_eq!(target.files.len(), 501);
    assert_eq!(target.omitted_file_count, 0);
    assert!(!target
        .limitations
        .contains(&"review_target_file_limit_exceeded".to_string()));
}

#[test]
fn review_target_marks_collection_budget_as_partial() {
    let mut pull_request = github_pull_request_from_value(&json!({
        "number": 42,
        "title": "Budget-sized review target",
        "state": "open",
        "head": { "ref": "feature", "sha": "2222222222222222222222222222222222222222" },
        "base": { "ref": "main", "sha": "1111111111111111111111111111111111111111" },
        "changed_files": 1000
    }));
    pull_request.changed_files = 1000;
    let files = (0..MAX_REVIEW_TARGET_LIST_ITEMS)
        .map(|index| ReviewPlatformFile {
            path: format!("src/file-{index}.rs"),
            old_path: None,
            status: ReviewFileStatus::Modified,
            additions: 1,
            deletions: 1,
            patch: Some("@@ -1 +1 @@\n-old\n+new".to_string()),
        })
        .collect();

    let target = review_target_from_parts(pull_request, files);

    assert_eq!(target.files.len(), MAX_REVIEW_TARGET_LIST_ITEMS);
    assert_eq!(target.omitted_file_count, 1);
    assert!(target
        .limitations
        .contains(&"provider_file_list_incomplete".to_string()));
}

#[test]
fn review_target_rejects_a_non_empty_but_truncated_provider_patch() {
    let mut pull_request = github_pull_request_from_value(&json!({
        "number": 42,
        "title": "Review target",
        "state": "open",
        "head": { "ref": "feature", "sha": "2222222222222222222222222222222222222222" },
        "base": { "ref": "main", "sha": "1111111111111111111111111111111111111111" },
        "changed_files": 1
    }));
    pull_request.changed_files = 1;

    let target = review_target_from_parts(
        pull_request,
        vec![ReviewPlatformFile {
            path: "src/lib.rs".to_string(),
            old_path: None,
            status: ReviewFileStatus::Modified,
            additions: 10,
            deletions: 10,
            patch: Some("@@ -1 +1 @@\n-old\n+new".to_string()),
        }],
    );

    assert!(!target.files[0].diff_available);
    assert!(target
        .limitations
        .contains(&"provider_file_diff_unavailable".to_string()));
}

#[test]
fn pull_request_file_diff_rejects_changed_head() {
    let pull_request = github_pull_request_from_value(&json!({
        "number": 42,
        "title": "Review target",
        "state": "open",
        "head": { "ref": "feature", "sha": "2222222222222222222222222222222222222222" },
        "base": { "ref": "main", "sha": "1111111111111111111111111111111111111111" }
    }));
    let result = file_diff_from_parts(
        pull_request,
        Vec::new(),
        "1111111111111111111111111111111111111111",
        "3333333333333333333333333333333333333333",
        "src/lib.rs",
    );

    assert!(matches!(result, Err(ReviewPlatformError::StaleTarget(_))));
}

#[test]
fn identity_file_diff_preserves_exact_expected_revisions() {
    let pull_request = github_pull_request_from_value(&json!({
        "number": 42,
        "title": "Review target",
        "state": "open",
        "head": { "sha": "2222222222222222222222222222222222222222" },
        "base": { "sha": "1111111111111111111111111111111111111111" }
    }));
    let diff = file_diff_from_parts(
        pull_request,
        vec![ReviewPlatformFile {
            path: "src/lib.rs".to_string(),
            old_path: None,
            status: ReviewFileStatus::Modified,
            additions: 1,
            deletions: 1,
            patch: Some("@@ -1 +1 @@\n-old\n+new".to_string()),
        }],
        "1111111111111111111111111111111111111111",
        "2222222222222222222222222222222222222222",
        "src/lib.rs",
    )
    .expect("identity diff should preserve exact revisions");

    assert_eq!(
        diff.base_revision,
        "1111111111111111111111111111111111111111"
    );
    assert_eq!(
        diff.head_revision,
        "2222222222222222222222222222222222222222"
    );
    assert!(diff.diff.contains("-old\n+new"));
}

#[test]
fn identity_file_diff_rejects_missing_or_unavailable_patch() {
    let pull_request = github_pull_request_from_value(&json!({
        "number": 42,
        "title": "Review target",
        "state": "open",
        "head": { "sha": "2222222222222222222222222222222222222222" },
        "base": { "sha": "1111111111111111111111111111111111111111" }
    }));
    let result = file_diff_from_parts(
        pull_request,
        vec![ReviewPlatformFile {
            path: "assets/image.png".to_string(),
            old_path: None,
            status: ReviewFileStatus::Modified,
            additions: 0,
            deletions: 0,
            patch: None,
        }],
        "1111111111111111111111111111111111111111",
        "2222222222222222222222222222222222222222",
        "assets/image.png",
    );

    assert!(matches!(result, Err(ReviewPlatformError::Api(_))));
}

#[tokio::test]
async fn identity_file_diff_service_rejects_invalid_provider_id_before_network() {
    let service =
        ReviewPlatformService::new_local_only(temp_token_store_path("identity-file-diff"));

    let result = service
        .pull_request_file_diff_by_identity(
            ReviewPlatformKind::Github,
            "github.com",
            "openai/example",
            "01",
            "1111111111111111111111111111111111111111",
            "2222222222222222222222222222222222222222",
            "src/lib.rs",
            None,
            None,
        )
        .await;

    assert!(matches!(result, Err(ReviewPlatformError::Api(_))));
}

#[test]
fn review_target_rejects_revisions_that_change_during_preparation() {
    let initial = github_pull_request_from_value(&json!({
        "number": 42,
        "title": "Review target",
        "state": "open",
        "head": { "sha": "2222222222222222222222222222222222222222" },
        "base": { "sha": "1111111111111111111111111111111111111111" }
    }));
    let confirmed = github_pull_request_from_value(&json!({
        "number": 42,
        "title": "Review target",
        "state": "open",
        "head": { "sha": "3333333333333333333333333333333333333333" },
        "base": { "sha": "1111111111111111111111111111111111111111" }
    }));

    assert!(matches!(
        ensure_pull_request_revisions_stable(&initial, &confirmed),
        Err(ReviewPlatformError::StaleTarget(_))
    ));
}

#[test]
fn identity_pr_target_plan_reuses_existing_provider_dispatch_without_workspace_git() {
    let tokens = ReviewPlatformAuthTokens::default();
    let identity = pull_request_identity_plan(
        ReviewPlatformKind::Github,
        "github.com",
        "example/repo",
        "42",
        &tokens,
    )
    .expect("public GitHub identity should not require a workspace remote");
    let existing_remote_context = provider_context(identity.context.remote.clone(), &tokens)
        .expect("existing remote context should be valid");

    assert_eq!(identity.pull_request_id, "42");
    assert_eq!(identity.context.remote.project_path, "example/repo");
    assert!(std::ptr::eq(
        provider_for(identity.context.remote.platform),
        provider_for(existing_remote_context.remote.platform),
    ));
}
