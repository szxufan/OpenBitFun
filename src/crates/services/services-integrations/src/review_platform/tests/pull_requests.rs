use super::super::*;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn spawn_empty_paginated_responses() -> (String, mpsc::Receiver<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock provider should bind");
    listener
        .set_nonblocking(true)
        .expect("mock provider should allow non-blocking accept");
    let address = listener.local_addr().expect("mock provider address");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut request_count = 0usize;
        let mut idle_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut request = [0u8; 4096];
                    let _ = stream.read(&mut request);
                    request_count += 1;
                    let next_page = request_count + 1;
                    let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Next-Page: {next_page}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]"
                        );
                    stream
                        .write_all(response.as_bytes())
                        .expect("mock provider response should write");
                    idle_deadline = Instant::now() + Duration::from_millis(300);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= idle_deadline {
                        let _ = sender.send(request_count);
                        return;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("mock provider pagination should accept: {error}"),
            }
        }
    });
    (format!("http://{address}"), receiver)
}

#[tokio::test]
async fn bounded_pagination_stops_empty_pages_with_next_links() {
    let (base_url, request_count) = spawn_empty_paginated_responses();
    let client = http_client().expect("review client should build");
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        fetch_bounded_paginated_array(
            |page| {
                let page = page.to_string();
                client
                    .get(&format!("{base_url}/items"))
                    .query(&[("page", &page)])
            },
            gitlab_next_page,
            MAX_REVIEW_TARGET_LIST_ITEMS,
        ),
    )
    .await
    .expect("bounded pagination must terminate independently of item progress");

    assert_eq!(
        result.expect("page budget should return partial evidence"),
        json!([])
    );
    assert!(
        request_count
            .recv_timeout(Duration::from_secs(3))
            .expect("mock provider should report request count")
            <= 32,
        "pagination request cap must remain small"
    );
}

#[test]
fn github_review_decision_uses_latest_review_per_author() {
    let reviews = json!([
        {
            "id": 1,
            "state": "CHANGES_REQUESTED",
            "user": { "login": "alice" }
        },
        {
            "id": 2,
            "state": "APPROVED",
            "user": { "login": "alice" }
        }
    ]);

    assert_eq!(github_review_decision(&reviews), ReviewDecision::Approved);
}

#[test]
fn github_pull_request_keeps_immutable_revisions() {
    let pull_request = github_pull_request_from_value(&json!({
        "number": 42,
        "title": "Review target",
        "state": "open",
        "user": { "login": "alice" },
        "head": { "ref": "feature", "sha": "2222222222222222222222222222222222222222" },
        "base": { "ref": "main", "sha": "1111111111111111111111111111111111111111" },
        "updated_at": "2026-07-11T00:00:00Z",
        "html_url": "https://github.com/example/repo/pull/42",
        "changed_files": 1
    }));

    assert_eq!(
        pull_request.base_revision.as_deref(),
        Some("1111111111111111111111111111111111111111")
    );
    assert_eq!(
        pull_request.head_revision.as_deref(),
        Some("2222222222222222222222222222222222222222")
    );
}

#[test]
fn pull_request_payload_without_known_flag_remains_backward_compatible() {
    let pull_request = gitcode_pull_request_from_value(&json!({
        "number": 5,
        "title": "legacy payload",
        "state": "open",
        "changes_count": "0"
    }));
    let mut value = serde_json::to_value(pull_request).expect("serialize pull request");
    value
        .as_object_mut()
        .expect("pull request object")
        .remove("changedFileCountKnown");

    let decoded: ReviewPlatformPullRequest =
        serde_json::from_value(value).expect("deserialize legacy pull request");

    assert!(decoded.changed_file_count_known);
    assert_eq!(decoded.changed_files, 0);
}

#[test]
fn github_cli_pull_request_maps_open_user_list_fields() {
    let pull_request = github_pull_request_from_gh_cli_value(
        &json!({
            "number": 42,
            "title": "Current user pull request",
            "state": "OPEN",
            "isDraft": false,
            "author": { "login": "octocat" },
            "headRefName": "feature",
            "headRefOid": "2222222222222222222222222222222222222222",
            "baseRefName": "main",
            "baseRefOid": "1111111111111111111111111111111111111111",
            "updatedAt": "2026-07-14T00:00:00Z",
            "url": "https://github.com/example/repo/pull/42",
            "additions": 12,
            "deletions": 3,
            "changedFiles": 2,
            "comments": [{ "id": "one" }, { "id": "two" }],
            "reviewDecision": "CHANGES_REQUESTED",
            "statusCheckRollup": [
                { "conclusion": "SUCCESS", "status": "COMPLETED" },
                { "conclusion": "FAILURE", "status": "COMPLETED" },
                { "conclusion": "", "status": "IN_PROGRESS" }
            ]
        }),
        Some("origin:github:example__repo"),
    );

    assert_eq!(pull_request.number, 42);
    assert_eq!(
        pull_request.provider_id.as_deref(),
        Some("origin:github:example__repo")
    );
    assert_eq!(pull_request.author, "octocat");
    assert_eq!(pull_request.state, ReviewItemState::Open);
    assert_eq!(
        pull_request.review_decision,
        ReviewDecision::ChangesRequested
    );
    assert_eq!(pull_request.comments, 2);
    assert_eq!(pull_request.checks.total, 3);
    assert_eq!(pull_request.checks.passed, 1);
    assert_eq!(pull_request.checks.failed, 1);
    assert_eq!(pull_request.checks.pending, 1);
}

#[test]
fn github_workspace_pull_requests_deduplicate_urls_not_numbers() {
    let make_pull_request = |provider_id: &str, url: &str, updated_at: &str| {
        github_pull_request_from_gh_cli_value(
            &json!({
                "number": 42,
                "title": provider_id,
                "state": "OPEN",
                "isDraft": false,
                "author": { "login": "octocat" },
                "headRefName": "feature",
                "baseRefName": "main",
                "updatedAt": updated_at,
                "url": url,
                "comments": [],
                "statusCheckRollup": []
            }),
            Some(provider_id),
        )
    };
    let page = aggregate_github_pull_requests(
        vec![
            make_pull_request(
                "fork",
                "https://github.com/example/fork/pull/42",
                "2026-07-14T01:00:00Z",
            ),
            make_pull_request(
                "upstream",
                "https://github.com/example/upstream/pull/42",
                "2026-07-14T02:00:00Z",
            ),
            make_pull_request(
                "duplicate-upstream",
                "https://github.com/example/upstream/pull/42",
                "2026-07-14T00:00:00Z",
            ),
        ],
        PullRequestPagination::new(Some(1), Some(10)),
    );

    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].provider_id.as_deref(), Some("upstream"));
    assert_eq!(page.items[1].provider_id.as_deref(), Some("fork"));
}

#[test]
fn github_review_decision_preserves_state_precedence() {
    let cases = [
        (
            "changes requested beats approved and commented",
            ["APPROVED", "COMMENTED", "CHANGES_REQUESTED"].as_slice(),
            ReviewDecision::ChangesRequested,
        ),
        (
            "approved beats commented",
            ["COMMENTED", "APPROVED"].as_slice(),
            ReviewDecision::Approved,
        ),
        (
            "commented beats unmatched pending state",
            ["PENDING", "COMMENTED"].as_slice(),
            ReviewDecision::Commented,
        ),
        (
            "unmatched state stays pending",
            ["PENDING"].as_slice(),
            ReviewDecision::Pending,
        ),
        (
            "no reviews stays pending",
            [].as_slice(),
            ReviewDecision::Pending,
        ),
    ];

    for (label, states, expected) in cases {
        let reviews = Value::Array(
            states
                .iter()
                .enumerate()
                .map(|(index, state)| {
                    json!({
                        "id": index,
                        "state": state,
                        "user": { "login": format!("reviewer-{index}") }
                    })
                })
                .collect(),
        );

        assert_eq!(github_review_decision(&reviews), expected, "{label}");
    }
}
