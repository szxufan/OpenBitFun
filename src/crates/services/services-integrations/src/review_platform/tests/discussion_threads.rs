use super::super::*;
use serde_json::json;

#[test]
fn github_threads_include_issue_comments_and_review_comments() {
    let reviews = json!([]);
    let review_comments = json!([
        {
            "id": 10,
            "path": "src/lib.rs",
            "line": 8,
            "user": { "login": "alice" },
            "body": "Inline comment",
            "updated_at": "2026-05-18T01:00:00Z"
        }
    ]);
    let issue_comments = json!([
        {
            "id": 20,
            "user": { "login": "bob" },
            "body": "Conversation comment",
            "updated_at": "2026-05-18T02:00:00Z"
        }
    ]);

    let threads = github_threads(&reviews, &review_comments, &issue_comments);

    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].id, "comment-10");
    assert_eq!(threads[0].file_path.as_deref(), Some("src/lib.rs"));
    assert_eq!(threads[1].id, "issue-comment-20");
    assert_eq!(threads[1].file_path, None);
    assert_eq!(threads[1].body, "Conversation comment");
}

#[test]
fn github_threads_keep_empty_body_reviews_visible() {
    let reviews = json!([
        {
            "id": 30,
            "state": "APPROVED",
            "user": { "login": "alice" },
            "body": "",
            "submitted_at": "2026-05-18T03:00:00Z"
        }
    ]);

    let threads = github_threads(&reviews, &json!([]), &json!([]));

    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id, "review-30");
    assert_eq!(threads[0].body, "Approved this pull request.");
}

#[test]
fn github_review_comment_replies_track_parent_comment() {
    let threads = github_threads(
        &json!([]),
        &json!([
            {
                "id": 40,
                "in_reply_to_id": 10,
                "user": { "login": "alice" },
                "body": "Reply",
                "updated_at": "2026-05-18T04:30:00Z"
            }
        ]),
        &json!([]),
    );

    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].kind, ReviewPlatformThreadKind::Comment);
    assert_eq!(
        threads[0].reply_to_provider_comment_id.as_deref(),
        Some("10")
    );
}

#[test]
fn gitlab_threads_include_top_level_notes_without_duplication() {
    let discussions = json!([
        {
            "id": "discussion-1",
            "resolved": false,
            "notes": [
                {
                    "id": "100",
                    "author": { "username": "alice" },
                    "body": "Inline note",
                    "updated_at": "2026-05-18T04:00:00Z",
                    "position": { "new_path": "src/lib.rs", "new_line": 12 }
                }
            ]
        }
    ]);
    let notes = json!([
        {
            "id": "100",
            "author": { "username": "alice" },
            "body": "Inline note",
            "updated_at": "2026-05-18T04:00:00Z",
            "position": { "new_path": "src/lib.rs", "new_line": 12 }
        },
        {
            "id": "200",
            "author": { "username": "bob" },
            "body": "Top-level note",
            "updated_at": "2026-05-18T05:00:00Z"
        }
    ]);

    let threads = gitlab_threads(&discussions, &notes);

    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].id, "discussion-discussion-1:note-100");
    assert_eq!(threads[1].id, "note-200");
    assert_eq!(threads[1].file_path, None);
    assert_eq!(threads[1].body, "Top-level note");
}

#[test]
fn gitlab_discussion_threads_mark_root_as_review_and_replies_as_comments() {
    let discussions = json!([
        {
            "id": "discussion-2",
            "resolved": false,
            "notes": [
                {
                    "id": "300",
                    "author": { "username": "alice" },
                    "body": "Root note",
                    "updated_at": "2026-05-18T06:00:00Z"
                },
                {
                    "id": "301",
                    "author": { "username": "bob" },
                    "body": "Reply note",
                    "updated_at": "2026-05-18T06:05:00Z"
                }
            ]
        }
    ]);

    let threads = gitlab_threads(&discussions, &json!([]));

    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].kind, ReviewPlatformThreadKind::Review);
    assert_eq!(threads[0].reply_to_provider_comment_id, None);
    assert_eq!(threads[1].kind, ReviewPlatformThreadKind::Comment);
    assert_eq!(
        threads[1].reply_to_provider_comment_id.as_deref(),
        Some("300")
    );
}
