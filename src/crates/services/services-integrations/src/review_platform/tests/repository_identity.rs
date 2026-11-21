use super::super::*;
use super::support::temp_token_store_path;
use tokio::fs;

#[test]
fn issue_mapping_identity_plan_normalizes_public_hosts_and_rejects_unsafe_input() {
    let tokens = ReviewPlatformAuthTokens::default();
    let public = provider_context_for_identity(
        ReviewPlatformKind::Github,
        " GitHub.COM. ",
        "example/repo.git",
        &tokens,
    )
    .expect("official GitHub should allow anonymous evidence reads");

    assert_eq!(public.remote.host, "github.com");
    assert_eq!(public.remote.project_path, "example/repo");
    assert_eq!(public.api_base_url, "https://api.github.com");
    assert!(public.token.is_none());

    assert!(provider_context_for_identity(
        ReviewPlatformKind::Github,
        "https://github.com",
        "example/repo",
        &tokens,
    )
    .is_err());
    assert!(provider_context_for_identity(
        ReviewPlatformKind::Github,
        "github.com",
        "example/../repo",
        &tokens,
    )
    .is_err());
}

#[test]
fn issue_mapping_identity_rejects_zero_and_leading_zero_ids() {
    for invalid in ["0", "00", "042", "-1", "+1", " 1"] {
        assert!(
            ProviderIssueIdentity::new(
                ReviewPlatformKind::Github,
                "github.com",
                "example/repo",
                invalid,
            )
            .is_err(),
            "{invalid:?} must be rejected"
        );
    }
    assert!(ProviderIssueIdentity::new(
        ReviewPlatformKind::Github,
        "github.com",
        "example/repo",
        "42",
    )
    .is_ok());
}

#[test]
fn issue_mapping_identity_plan_requires_stored_token_for_non_public_hosts() {
    let mut tokens = ReviewPlatformAuthTokens::default();
    assert!(provider_context_for_identity(
        ReviewPlatformKind::Gitlab,
        "gitlab.example.internal",
        "group/repo",
        &tokens,
    )
    .is_err());

    tokens.tokens.insert(
        token_key(ReviewPlatformKind::Gitlab, "gitlab.example.internal")
            .expect("token key should be valid"),
        "stored-token".to_string(),
    );
    let trusted = provider_context_for_identity(
        ReviewPlatformKind::Gitlab,
        "gitlab.example.internal",
        "group/repo",
        &tokens,
    )
    .expect("stored token should authorize an existing self-hosted provider context");

    assert_eq!(
        trusted.api_base_url,
        "https://gitlab.example.internal/api/v4"
    );
    assert_eq!(trusted.token.as_deref(), Some("stored-token"));
}

#[test]
fn existing_self_hosted_github_remote_ignores_stored_tokens() {
    let host = "github.example.internal";
    let mut tokens = ReviewPlatformAuthTokens::default();
    tokens.tokens.insert(
        token_key(ReviewPlatformKind::Github, host).expect("token key should be valid"),
        "stored-token".to_string(),
    );
    let identity_context =
        provider_context_for_identity(ReviewPlatformKind::Github, host, "example/repo", &tokens)
            .expect("GitHub CLI should authorize a self-hosted GitHub context");

    let existing_remote_context = provider_context(identity_context.remote, &tokens)
        .expect("existing remote context should remain valid");

    assert_eq!(
        existing_remote_context.api_base_url,
        "https://github.example.internal/api/v3"
    );
    assert_eq!(existing_remote_context.token, None);
    assert_eq!(
        existing_remote_context.remote.auth_source,
        ReviewAuthSource::GhCli
    );
}

#[test]
fn remote_detection_rejects_brand_substring_attacker_hosts() {
    let remote = parse_remote(
        "origin",
        "https://github.com.attacker/example/repo.git",
        &ReviewPlatformAuthTokens::default(),
    )
    .expect("syntactically valid Git remote should be represented");

    assert_eq!(remote.platform, ReviewPlatformKind::Unknown);
    assert!(!remote.supported);
}

#[test]
fn workspace_remote_identity_matcher_requires_exact_host_and_project() {
    assert!(remote_url_matches_provider_identity(
        "https://code.company.internal/group/repo.git",
        ReviewPlatformKind::Gitlab,
        "code.company.internal",
        "group/repo",
    ));
    assert!(!remote_url_matches_provider_identity(
        "https://other.company.internal/group/repo.git",
        ReviewPlatformKind::Gitlab,
        "code.company.internal",
        "group/repo",
    ));
    assert!(!remote_url_matches_provider_identity(
        "https://code.company.internal/other/repo.git",
        ReviewPlatformKind::Gitlab,
        "code.company.internal",
        "group/repo",
    ));
    assert!(!remote_url_matches_provider_identity(
        "https://github.com.attacker/example/repo.git",
        ReviewPlatformKind::Github,
        "github.com",
        "example/repo",
    ));
    assert!(remote_url_matches_provider_identity(
        "https://github.com.attacker/example/repo.git",
        ReviewPlatformKind::Github,
        "github.com.attacker",
        "example/repo",
    ));
}

#[tokio::test]
async fn remote_detection_uses_exact_token_authority_for_internal_provider_kind() {
    let repository = tempfile::tempdir().expect("temporary repository should be created");
    let repository_path = repository
        .path()
        .to_str()
        .expect("repository path should be UTF-8");
    execute_git_command(repository_path, &["init"])
        .await
        .expect("temporary repository should initialize");
    execute_git_command(
        repository_path,
        &[
            "remote",
            "add",
            "origin",
            "https://code.company.internal/group/repo.git",
        ],
    )
    .await
    .expect("internal remote should be added");
    let token_path = temp_token_store_path("internal-provider-authority");
    let service = ReviewPlatformService::new_local_only(token_path.clone());
    service
        .update_auth_token(
            ReviewPlatformKind::Gitlab,
            "code.company.internal",
            "internal-token",
        )
        .await
        .expect("exact GitLab authority should be stored");

    let remotes = service
        .discover_remotes(&ReviewRepositoryLocator::legacy_path(repository_path))
        .await
        .expect("registered internal remote should be discovered");

    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].platform, ReviewPlatformKind::Gitlab);
    assert_eq!(remotes[0].host, "code.company.internal");
    assert!(remotes[0].supported);
    let _ = fs::remove_file(token_path).await;
}

#[tokio::test]
async fn identity_trust_accepts_exact_discovered_self_hosted_remote_without_token() {
    let repository = tempfile::tempdir().expect("temporary repository should be created");
    execute_git_command(
        repository
            .path()
            .to_str()
            .expect("repository path should be UTF-8"),
        &["init"],
    )
    .await
    .expect("temporary repository should initialize");
    execute_git_command(
        repository
            .path()
            .to_str()
            .expect("repository path should be UTF-8"),
        &[
            "remote",
            "add",
            "origin",
            "https://gitlab.example.internal/group/repo.git",
        ],
    )
    .await
    .expect("trusted remote should be added");
    let service = ReviewPlatformService::new_local_only(temp_token_store_path("trust"));
    let tokens = ReviewPlatformAuthTokens::default();

    let context = service
        .provider_context_for_identity_request(
            ReviewPlatformKind::Gitlab,
            "gitlab.example.internal",
            "group/repo",
            Some(&ReviewRepositoryLocator::legacy_path(
                repository
                    .path()
                    .to_str()
                    .expect("repository path should be UTF-8"),
            )),
            &tokens,
        )
        .await
        .expect("exact discovered remote should establish anonymous trust");

    assert!(context.token.is_none());
    assert_eq!(context.remote.project_path, "group/repo");
}

#[tokio::test]
async fn identity_trust_rejects_unrelated_workspace_remote_without_token() {
    let repository = tempfile::tempdir().expect("temporary repository should be created");
    execute_git_command(
        repository
            .path()
            .to_str()
            .expect("repository path should be UTF-8"),
        &["init"],
    )
    .await
    .expect("temporary repository should initialize");
    execute_git_command(
        repository
            .path()
            .to_str()
            .expect("repository path should be UTF-8"),
        &[
            "remote",
            "add",
            "origin",
            "https://gitlab.example.internal/other/repo.git",
        ],
    )
    .await
    .expect("unrelated remote should be added");
    let service = ReviewPlatformService::new_local_only(temp_token_store_path("untrusted"));
    let tokens = ReviewPlatformAuthTokens::default();

    let result = service
        .provider_context_for_identity_request(
            ReviewPlatformKind::Gitlab,
            "gitlab.example.internal",
            "group/repo",
            Some(&ReviewRepositoryLocator::legacy_path(
                repository
                    .path()
                    .to_str()
                    .expect("repository path should be UTF-8"),
            )),
            &tokens,
        )
        .await;

    assert!(matches!(result, Err(ReviewPlatformError::Api(_))));
}

#[tokio::test]
async fn identity_trust_accepts_stored_token_without_workspace_path() {
    let service = ReviewPlatformService::new_local_only(temp_token_store_path("token-trust"));
    let mut tokens = ReviewPlatformAuthTokens::default();
    tokens.tokens.insert(
        token_key(ReviewPlatformKind::Gitlab, "gitlab.example.internal")
            .expect("token key should normalize"),
        "stored-token".to_string(),
    );

    let context = service
        .provider_context_for_identity_request(
            ReviewPlatformKind::Gitlab,
            "gitlab.example.internal",
            "group/repo",
            None,
            &tokens,
        )
        .await
        .expect("stored token should authorize without workspace trust");

    assert_eq!(context.token.as_deref(), Some("stored-token"));
}
