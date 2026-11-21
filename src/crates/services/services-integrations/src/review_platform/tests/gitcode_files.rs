use super::super::*;
use serde_json::json;

#[test]
fn gitcode_pull_request_maps_documented_change_counts() {
    let pull_request = gitcode_pull_request_from_value(&json!({
        "number": 5,
        "title": "fix bugs",
        "state": "open",
        "added_lines": 133,
        "removed_lines": 22,
        "changes_count": "7",
        "additions": 99,
        "deletions": 99,
        "changed_files": 99
    }));

    assert_eq!(pull_request.additions, 133);
    assert_eq!(pull_request.deletions, 22);
    assert_eq!(pull_request.changed_files, 7);
    assert!(pull_request.changed_file_count_known);
}

#[test]
fn gitcode_pull_request_marks_approximate_change_count_unknown() {
    let pull_request = gitcode_pull_request_from_value(&json!({
        "number": 5,
        "title": "large change",
        "state": "open",
        "changes_count": "3000+"
    }));

    assert_eq!(pull_request.changed_files, 3_000);
    assert!(!pull_request.changed_file_count_known);
}

#[test]
fn gitcode_pull_request_marks_missing_file_count_unknown() {
    let pull_request = gitcode_pull_request_from_value(&json!({
        "number": 5,
        "title": "fix bugs",
        "state": "open",
        "added_lines": 133,
        "removed_lines": 22
    }));

    assert_eq!(pull_request.changed_files, 0);
    assert!(!pull_request.changed_file_count_known);
    assert_eq!(pull_request.additions, 133);
    assert_eq!(pull_request.deletions, 22);
}

#[test]
fn gitcode_file_maps_nested_patch_and_change_flags() {
    let file = gitcode_file_from_value(&json!({
        "filename": "src/new.rs",
        "old_path": "src/old.rs",
        "status": null,
        "new_file": false,
        "renamed_file": true,
        "deleted_file": false,
        "additions": 1,
        "deletions": 1,
        "patch": { "diff": "@@ -1 +1 @@\n-old\n+new" }
    }));

    assert_eq!(file.path, "src/new.rs");
    assert_eq!(file.old_path.as_deref(), Some("src/old.rs"));
    assert_eq!(file.status, ReviewFileStatus::Renamed);
    assert_eq!(file.patch.as_deref(), Some("@@ -1 +1 @@\n-old\n+new"));
    assert!(file_has_complete_patch(&file));
}

#[test]
fn gitcode_capped_files_pagination_does_not_claim_exact_total() {
    let pagination = gitcode_files_pagination(
        PullRequestPagination {
            page: 60,
            per_page: 50,
        },
        GITCODE_PULL_REQUEST_FILES_RESPONSE_LIMIT,
    );

    assert_eq!(pagination.total, None);
    assert!(!pagination.has_next);
}

#[test]
fn gitcode_review_target_reports_the_files_the_budget_left_out() {
    let mut pull_request = gitcode_pull_request_from_value(&json!({
        "number": 5,
        "title": "large change",
        "state": "open",
        "added_lines": 9_999,
        "removed_lines": 8_888,
        "changes_count": "2500"
    }));
    let response = JsonResponse {
        value: Value::Array(
            (0..2_500)
                .map(|index| {
                    json!({
                        "filename": format!("src/file-{index}.rs"),
                        "additions": "1",
                        "deletions": "2"
                    })
                })
                .collect(),
        ),
        headers: ReviewHttpHeaders::default(),
    };
    let files = array_items(&response.value)
        .iter()
        .take(MAX_REVIEW_TARGET_LIST_ITEMS)
        .map(gitcode_file_from_value)
        .collect::<Vec<_>>();

    apply_gitcode_pull_request_change_stats(&mut pull_request, &response);
    let target = review_target_from_parts(pull_request, files);

    assert_eq!(target.pull_request.changed_files, 2_500);
    assert!(target.pull_request.changed_file_count_known);
    assert_eq!(target.pull_request.additions, 2_500);
    assert_eq!(target.pull_request.deletions, 5_000);
    assert_eq!(target.files.len(), MAX_REVIEW_TARGET_LIST_ITEMS);
    assert_eq!(target.omitted_file_count, 1_500);
    assert!(target
        .limitations
        .contains(&"provider_file_list_incomplete".to_string()));
}

#[test]
fn gitcode_files_response_too_large_reports_explicit_reason() {
    let error = gitcode_files_http_error(ReviewHttpError::ResponseTooLarge {
        limit_bytes: GITCODE_PULL_REQUEST_FILES_RESPONSE_BYTES,
    });

    assert_eq!(
        error.to_string(),
        "Provider API failed: GitCode pull request files response exceeded the 16777216-byte limit"
    );
}

#[test]
fn gitcode_file_response_overrides_incomplete_list_stats() {
    let mut pull_request = gitcode_pull_request_from_value(&json!({
        "number": 5,
        "title": "fix bugs",
        "state": "open",
        "added_lines": 133,
        "removed_lines": 22
    }));
    let response = JsonResponse {
        value: json!([
            { "filename": "src/a.rs", "additions": "5", "deletions": "2" },
            { "filename": "src/b.rs", "additions": "8", "deletions": "3" }
        ]),
        headers: ReviewHttpHeaders::default(),
    };

    apply_gitcode_pull_request_change_stats(&mut pull_request, &response);

    assert_eq!(pull_request.changed_files, 2);
    assert!(pull_request.changed_file_count_known);
    assert_eq!(pull_request.additions, 13);
    assert_eq!(pull_request.deletions, 5);
}

#[test]
fn gitcode_empty_file_response_confirms_zero_files() {
    let mut pull_request = gitcode_pull_request_from_value(&json!({
        "number": 5,
        "title": "no changes",
        "state": "open"
    }));
    let response = JsonResponse {
        value: json!([]),
        headers: ReviewHttpHeaders::default(),
    };

    apply_gitcode_pull_request_change_stats(&mut pull_request, &response);

    assert_eq!(pull_request.changed_files, 0);
    assert!(pull_request.changed_file_count_known);
}

#[test]
fn gitcode_non_array_file_response_does_not_fake_zero_files() {
    let mut pull_request = gitcode_pull_request_from_value(&json!({
        "number": 5,
        "title": "fix bugs",
        "state": "open",
        "added_lines": 133,
        "removed_lines": 22
    }));
    let response = JsonResponse {
        value: json!({ "message": "temporarily unavailable" }),
        headers: ReviewHttpHeaders::default(),
    };

    apply_gitcode_pull_request_change_stats(&mut pull_request, &response);

    assert_eq!(pull_request.changed_files, 0);
    assert!(!pull_request.changed_file_count_known);
    assert_eq!(pull_request.additions, 133);
    assert_eq!(pull_request.deletions, 22);
}

#[test]
fn gitcode_file_response_prefers_total_line_headers() {
    let mut pull_request = gitcode_pull_request_from_value(&json!({
        "number": 5,
        "title": "fix bugs",
        "state": "open"
    }));
    let response = JsonResponse {
        value: json!([
            { "filename": "src/a.rs", "additions": "5", "deletions": "2" },
            { "filename": "src/b.rs", "additions": "8", "deletions": "3" }
        ]),
        headers: ReviewHttpHeaders::from_pairs(&[
            ("total_added_lines", "133"),
            ("total_removed_lines", "22"),
        ]),
    };

    apply_gitcode_pull_request_change_stats(&mut pull_request, &response);

    assert_eq!(pull_request.changed_files, 2);
    assert!(pull_request.changed_file_count_known);
    assert_eq!(pull_request.additions, 133);
    assert_eq!(pull_request.deletions, 22);
}

#[test]
fn gitcode_capped_file_response_does_not_claim_truncated_stats() {
    let response = JsonResponse {
        value: Value::Array(
            (0..GITCODE_PULL_REQUEST_FILES_RESPONSE_LIMIT)
                .map(|index| {
                    json!({
                        "filename": format!("src/{index}.rs"),
                        "additions": "1",
                        "deletions": "1"
                    })
                })
                .collect(),
        ),
        headers: ReviewHttpHeaders::from_pairs(&[
            ("total_added_lines", "1910"),
            ("total_removed_lines", "116799"),
        ]),
    };
    let mut unknown_count = gitcode_pull_request_from_value(&json!({
        "number": 5,
        "title": "large change",
        "state": "open",
        "added_lines": 133,
        "removed_lines": 22
    }));

    apply_gitcode_pull_request_change_stats(&mut unknown_count, &response);

    assert_eq!(unknown_count.changed_files, 3_000);
    assert!(!unknown_count.changed_file_count_known);
    assert_eq!(unknown_count.additions, 133);
    assert_eq!(unknown_count.deletions, 22);

    let mut provider_count = gitcode_pull_request_from_value(&json!({
        "number": 5,
        "title": "large change",
        "state": "open",
        "added_lines": 401011,
        "removed_lines": 5219754,
        "changes_count": "32202"
    }));

    apply_gitcode_pull_request_change_stats(&mut provider_count, &response);

    assert_eq!(provider_count.changed_files, 32_202);
    assert!(provider_count.changed_file_count_known);
    assert_eq!(provider_count.additions, 401_011);
    assert_eq!(provider_count.deletions, 5_219_754);
}
