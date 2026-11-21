use super::super::*;
use super::support::temp_token_store_path;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::fs;

#[tokio::test]
async fn token_store_uses_injected_path() {
    let path = temp_token_store_path("token-store");
    let service = ReviewPlatformService::new_local_only(path.clone());

    service
        .update_auth_token(ReviewPlatformKind::Gitlab, "GitLab.com", " secret-token ")
        .await
        .expect("token update should write injected store");

    let tokens = service
        .load_stored_tokens()
        .await
        .expect("stored token file should be readable");
    assert_eq!(
        tokens.get(ReviewPlatformKind::Gitlab, "gitlab.com"),
        Some("secret-token")
    );
    assert!(path.exists());

    let _ = fs::remove_file(path).await;
}

#[tokio::test]
async fn github_token_actions_are_rejected() {
    let path = temp_token_store_path("github-token-rejected");
    let service = ReviewPlatformService::new_local_only(path.clone());

    let update = service
        .update_auth_token(ReviewPlatformKind::Github, "github.com", "secret-token")
        .await;
    let clear = service
        .clear_auth_token(ReviewPlatformKind::Github, "github.com")
        .await;

    assert!(
        matches!(update, Err(ReviewPlatformError::Api(message)) if message.contains("gh auth login"))
    );
    assert!(
        matches!(clear, Err(ReviewPlatformError::Api(message)) if message.contains("gh auth logout"))
    );
    assert!(!path.exists());
}

#[tokio::test]
async fn pre_openbitfun_github_token_is_rejected_without_modifying_store() {
    let path = temp_token_store_path("pre-openbitfun-github-token");
    let original = serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "tokens": {
            "github:github.com": {
                "token": "pre-openbitfun-github-token",
                "updatedAt": "2026-07-14T00:00:00Z"
            },
            "gitlab:gitlab.com": {
                "token": "gitlab-token",
                "updatedAt": "2026-07-14T00:00:00Z"
            }
        }
    }))
    .expect("token fixture should serialize");
    fs::write(&path, &original)
        .await
        .expect("token fixture should be written");
    let service = ReviewPlatformService::new_local_only(path.clone());

    let error = service
        .load_stored_tokens()
        .await
        .expect_err("pre-OpenBitFun GitHub token must require explicit migration");

    assert!(error.to_string().contains("pre-OpenBitFun"), "{error}");
    assert!(error.to_string().contains("migration tool"), "{error}");
    assert_eq!(
        fs::read(&path).await.expect("read unchanged store"),
        original
    );
    let _ = fs::remove_file(path).await;
}

#[test]
fn gh_auth_status_uses_only_the_active_successful_account() {
    let status = parse_gh_auth_status(
        "github.com",
        &json!({
            "hosts": {
                "github.com": [
                    { "state": "failure", "active": false, "login": "old" },
                    { "state": "success", "active": true, "login": "octocat" }
                ]
            }
        }),
    );

    assert!(status.installed);
    assert!(status.connected);
    assert_eq!(status.username.as_deref(), Some("octocat"));
}

#[tokio::test]
async fn token_store_preserves_io_path_case_and_only_normalizes_lock_identity() {
    let base = temp_token_store_path("mixed-case-path");
    let mixed_path = base.with_file_name("MiXeD-Review-Token-Store.JSON");
    let service = ReviewPlatformService::new_local_only(mixed_path.clone());

    assert_eq!(service.token_store_path(), mixed_path.as_path());
    service
        .update_auth_token(ReviewPlatformKind::Gitlab, "gitlab.com", "case-token")
        .await
        .expect("mixed-case token path should be writable");
    let created_names = std::fs::read_dir(
        mixed_path
            .parent()
            .expect("mixed-case token path should have a parent"),
    )
    .expect("mixed-case token directory should be readable")
    .filter_map(Result::ok)
    .map(|entry| entry.file_name())
    .collect::<Vec<_>>();
    assert!(created_names.iter().any(|name| {
        name == mixed_path
            .file_name()
            .expect("mixed-case token path should have a file name")
    }));

    let case_variant = PathBuf::from(mixed_path.to_string_lossy().to_ascii_uppercase());
    let variant_service = ReviewPlatformService::new_local_only(case_variant);
    #[cfg(windows)]
    assert!(Arc::ptr_eq(
        &service.token_store_lock,
        &variant_service.token_store_lock
    ));
    #[cfg(not(windows))]
    assert!(!Arc::ptr_eq(
        &service.token_store_lock,
        &variant_service.token_store_lock
    ));

    let _ = fs::remove_file(mixed_path).await;
}

#[tokio::test]
async fn token_authority_normalization_is_shared_by_save_get_and_clear() {
    let path = temp_token_store_path("token-authority-normalization");
    let service = ReviewPlatformService::new_local_only(path.clone());

    service
        .update_auth_token(ReviewPlatformKind::Gitlab, " GitLab.COM. ", "secret-token")
        .await
        .expect("normalized token should save");
    let tokens = service
        .load_stored_tokens()
        .await
        .expect("normalized token should load");
    assert_eq!(
        tokens.get(ReviewPlatformKind::Gitlab, "gitlab.com."),
        Some("secret-token")
    );

    service
        .clear_auth_token(ReviewPlatformKind::Gitlab, "GITLAB.COM.")
        .await
        .expect("normalized token should clear");
    let tokens = service
        .load_stored_tokens()
        .await
        .expect("cleared token store should load");
    assert_eq!(tokens.get(ReviewPlatformKind::Gitlab, "gitlab.com"), None);
    let _ = fs::remove_file(path).await;
}

#[tokio::test]
async fn noncanonical_stored_token_authority_is_rejected_without_modification() {
    let path = temp_token_store_path("noncanonical-token-authority");
    let original = serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "tokens": {
            "gitlab:GitLab.COM.": {
                "token": "noncanonical-token",
                "updatedAt": "2026-07-11T00:00:00Z"
            }
        }
    }))
    .expect("noncanonical token fixture should serialize");
    fs::write(&path, &original)
        .await
        .expect("noncanonical token fixture should be written");
    let service = ReviewPlatformService::new_local_only(path.clone());

    let error = service
        .load_stored_tokens()
        .await
        .expect_err("noncanonical token authority must require explicit migration");

    assert!(error.to_string().contains("not canonical"), "{error}");
    assert!(error.to_string().contains("migration tool"), "{error}");
    assert_eq!(
        fs::read(&path).await.expect("read unchanged store"),
        original
    );
    let _ = fs::remove_file(path).await;
}

#[tokio::test]
async fn canonical_and_noncanonical_token_conflict_is_not_rewritten() {
    let path = temp_token_store_path("noncanonical-token-conflict");
    let original = serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "tokens": {
            "gitlab:GitLab.COM.": {
                "token": "noncanonical-token",
                "updatedAt": "2026-07-12T00:00:00Z"
            },
            "gitlab:gitlab.com": {
                "token": "canonical-token",
                "updatedAt": "2026-07-11T00:00:00Z"
            }
        }
    }))
    .expect("conflicting token fixture should serialize");
    fs::write(&path, &original)
        .await
        .expect("conflicting token fixture should be written");
    let service = ReviewPlatformService::new_local_only(path.clone());

    let load_error = service
        .load_stored_tokens()
        .await
        .expect_err("conflicting token store must not be normalized on load");
    assert!(load_error.to_string().contains("not canonical"));

    let update_error = service
        .update_auth_token(ReviewPlatformKind::Gitlab, "GITLAB.COM.", "new-token")
        .await
        .expect_err("write operations must not repair a noncanonical store");
    assert!(update_error.to_string().contains("not canonical"));
    assert_eq!(
        fs::read(&path).await.expect("read unchanged store"),
        original
    );
    let _ = fs::remove_file(path).await;
}

#[tokio::test]
async fn independent_owners_concurrently_update_without_losing_hosts() {
    let path = temp_token_store_path("concurrent-token-updates");
    let first = ReviewPlatformService::new_local_only(path.clone());
    let second = ReviewPlatformService::new_local_only(path.clone());

    let (gitcode, gitlab) = tokio::join!(
        first.update_auth_token(ReviewPlatformKind::Gitcode, "gitcode.com", "gitcode-token"),
        second.update_auth_token(ReviewPlatformKind::Gitlab, "gitlab.com", "gitlab-token")
    );
    gitcode.expect("GitCode token update should succeed");
    gitlab.expect("GitLab token update should succeed");

    let stored = first
        .load_stored_tokens()
        .await
        .expect("concurrent token store should load");
    assert_eq!(
        stored.get(ReviewPlatformKind::Gitcode, "gitcode.com"),
        Some("gitcode-token")
    );
    assert_eq!(
        stored.get(ReviewPlatformKind::Gitlab, "gitlab.com"),
        Some("gitlab-token")
    );
    let _ = fs::remove_file(path).await;
}

#[tokio::test]
async fn independent_owners_concurrently_update_and_clear_without_lost_mutations() {
    let path = temp_token_store_path("concurrent-token-update-clear");
    let first = ReviewPlatformService::new_local_only(path.clone());
    let second = ReviewPlatformService::new_local_only(path.clone());
    first
        .update_auth_token(ReviewPlatformKind::Gitcode, "gitcode.com", "old-gitcode")
        .await
        .expect("GitCode fixture token should save");
    first
        .update_auth_token(ReviewPlatformKind::Gitlab, "gitlab.com", "old-gitlab")
        .await
        .expect("GitLab fixture token should save");

    let (update, clear) = tokio::join!(
        first.update_auth_token(ReviewPlatformKind::Gitcode, "gitcode.com", "new-gitcode"),
        second.clear_auth_token(ReviewPlatformKind::Gitlab, "gitlab.com")
    );
    update.expect("GitCode token update should succeed");
    clear.expect("GitLab token clear should succeed");

    let stored = second
        .load_stored_tokens()
        .await
        .expect("updated token store should load");
    assert_eq!(
        stored.get(ReviewPlatformKind::Gitcode, "gitcode.com"),
        Some("new-gitcode")
    );
    assert_eq!(stored.get(ReviewPlatformKind::Gitlab, "gitlab.com"), None);
    let temp_prefix = format!(
        ".{}.",
        path.file_name()
            .expect("token path should have a file name")
            .to_string_lossy()
    );
    let mut entries = fs::read_dir(
        path.parent()
            .expect("temporary token path should have a parent"),
    )
    .await
    .expect("temporary token directory should be readable");
    while let Some(entry) = entries
        .next_entry()
        .await
        .expect("temporary token entry should be readable")
    {
        assert!(!entry
            .file_name()
            .to_string_lossy()
            .starts_with(&temp_prefix));
    }
    let _ = fs::remove_file(path).await;
}

#[test]
fn token_store_lock_registry_reclaims_dead_paths() {
    let marker = format!(
        "registry-reclaim-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos()
    );
    for index in 0..64 {
        let path = std::env::temp_dir().join(format!("{marker}-{index}.json"));
        drop(ReviewPlatformService::new_local_only(path));
    }
    let survivor_path = std::env::temp_dir().join(format!("{marker}-survivor.json"));
    let _survivor = ReviewPlatformService::new_local_only(survivor_path);

    assert_eq!(token_store_lock_registry_entries_for_test(&marker), 1);
}
