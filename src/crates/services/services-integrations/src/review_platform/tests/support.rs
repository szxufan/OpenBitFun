use super::super::*;
use serde_json::json;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn temp_token_store_path(name: &str) -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "openbitfun-review-platform-{name}-{}-{id}.json",
        std::process::id()
    ))
}

pub(super) fn github_issue_fixture() -> Value {
    json!({
        "number": 42,
        "html_url": "https://github.com/example/repo/issues/42",
        "title": "Provider Issue evidence",
        "body": "Keep the complete Issue body.",
        "state": "open",
        "user": { "login": "alice" },
        "labels": [
            { "name": "review" },
            { "name": "bug" }
        ],
        "created_at": "2026-07-11T00:00:00Z",
        "updated_at": "2026-07-11T01:00:00Z"
    })
}

pub(super) fn github_issue_identity() -> ProviderIssueIdentity {
    ProviderIssueIdentity::new(
        ReviewPlatformKind::Github,
        "github.com",
        "example/repo",
        "42",
    )
    .expect("GitHub identity fixture should be valid")
}
