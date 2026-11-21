use super::super::*;
use super::support::temp_token_store_path;
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::fs;

#[test]
fn git_ownership_rejection_keeps_a_stable_repository_trust_contract() {
    let detail = concat!(
        "fatal: detected dubious ownership in repository at '/srv/shared/repo'\n",
        "To add an exception for this directory, call:\n",
        "git config --global --add safe.directory /srv/shared/repo",
    );

    let error = classify_git_command_failure("/srv/controller/path", detail.to_string());

    assert!(matches!(
        error,
        ReviewPlatformError::RepositoryUntrusted {
            ref repository_path,
            detail: ref captured_detail,
        } if repository_path == "/srv/shared/repo" && captured_detail == detail
    ));
    assert_eq!(error.untrusted_repository_path(), Some("/srv/shared/repo"));
    assert_eq!(
        untrusted_repository_error_message("/srv/shared/repo"),
        "git_repository_untrusted: /srv/shared/repo"
    );
}

#[test]
fn ordinary_git_failure_remains_an_invalid_repository_error() {
    let error =
        classify_git_command_failure("/srv/project", "fatal: not a git repository".to_string());

    assert!(matches!(
        error,
        ReviewPlatformError::InvalidRepository(ref detail)
            if detail == "fatal: not a git repository"
    ));
}

struct AlwaysRemoteWorkspace;

#[async_trait::async_trait]
impl ReviewPlatformWorkspaceClassifier for AlwaysRemoteWorkspace {
    async fn classify_repository(
        &self,
        repository: &ReviewRepositoryLocator,
    ) -> Result<ReviewGitExecution, ReviewPlatformError> {
        Ok(ReviewGitExecution::Remote(ReviewRemoteGitTarget {
            workspace_id: repository
                .workspace_id
                .clone()
                .unwrap_or_else(|| "remote-workspace".to_string()),
            connection_id: "conn-1".to_string(),
        }))
    }
}

/// Remote workspace runtime stub that records routed git commands and
/// answers repository probes for a GitLab-hosted remote repository.
#[derive(Default)]
struct RecordingRemoteGitRuntime {
    commands: std::sync::Mutex<Vec<(String, String, Vec<String>)>>,
}

#[async_trait::async_trait]
impl ReviewPlatformWorkspaceClassifier for RecordingRemoteGitRuntime {
    async fn classify_repository(
        &self,
        repository: &ReviewRepositoryLocator,
    ) -> Result<ReviewGitExecution, ReviewPlatformError> {
        Ok(ReviewGitExecution::Remote(ReviewRemoteGitTarget {
            workspace_id: repository
                .workspace_id
                .clone()
                .unwrap_or_else(|| "remote-workspace".to_string()),
            connection_id: "conn-1".to_string(),
        }))
    }

    async fn execute_remote_git_command(
        &self,
        target: &ReviewRemoteGitTarget,
        current_dir: &str,
        args: &[&str],
    ) -> Result<String, ReviewPlatformError> {
        self.commands.lock().expect("commands lock").push((
            target.workspace_id.clone(),
            current_dir.to_string(),
            args.iter().map(|arg| arg.to_string()).collect(),
        ));
        match args {
            ["rev-parse", "--show-toplevel"] => Ok("/srv/projects\n".to_string()),
            ["remote", "-v"] => Ok(concat!(
                "origin\thttps://gitlab.com/example/repo.git (fetch)\n",
                "origin\thttps://gitlab.com/example/repo.git (push)\n",
            )
            .to_string()),
            _ => Err(ReviewPlatformError::InvalidRepository(format!(
                "unexpected remote git command: {args:?}"
            ))),
        }
    }
}

#[test]
fn git_execution_errors_distinguish_missing_git_from_workspace_and_permission_failures() {
    let current_dir = std::env::temp_dir();
    let missing_dir = temp_token_store_path("missing-workspace");
    let error = git_execution_error(
        &current_dir,
        std::io::Error::from(std::io::ErrorKind::NotFound),
    );
    assert!(matches!(error, ReviewPlatformError::GitUnavailable));
    assert!(error.to_string().starts_with("git_unavailable:"));
    assert!(!error.to_string().contains("Invalid repository path"));

    let error = git_execution_error(
        &missing_dir,
        std::io::Error::from(std::io::ErrorKind::NotFound),
    );
    assert!(matches!(error, ReviewPlatformError::InvalidRepository(_)));

    let error = git_execution_error(
        &current_dir,
        std::io::Error::from(std::io::ErrorKind::PermissionDenied),
    );
    assert!(matches!(error, ReviewPlatformError::InvalidRepository(_)));
}

#[tokio::test]
async fn workspace_snapshot_fails_loudly_when_remote_git_execution_is_not_wired() {
    let path = temp_token_store_path("remote-classifier");
    let service = ReviewPlatformService::new(path, Arc::new(AlwaysRemoteWorkspace));

    let error = service
        .workspace_snapshot(
            &ReviewRepositoryLocator::new(Some("ws-remote".to_string()), "not-a-git-repository"),
            None,
            None,
            None,
        )
        .await
        .expect_err("remote workspace without remote git wiring must fail loudly");

    assert!(
        error
            .to_string()
            .contains("Remote workspace Git execution is not wired"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn workspace_context_routes_git_probes_through_remote_runtime() {
    let path = temp_token_store_path("remote-git-routing");
    let runtime = Arc::new(RecordingRemoteGitRuntime::default());
    let service = ReviewPlatformService::new(path, runtime.clone());

    let snapshot = service
        .workspace_context(
            &ReviewRepositoryLocator::new(
                Some("ws-remote".to_string()),
                "/srv/projects/openbitfun",
            ),
            None,
        )
        .await
        .expect("remote workspace context should resolve through remote git");

    assert_eq!(
        snapshot
            .repository
            .as_ref()
            .map(|repository| repository.project_path.as_str()),
        Some("example/repo")
    );
    assert_eq!(
        snapshot
            .repository
            .as_ref()
            .and_then(|repository| repository.workspace_path.as_deref()),
        Some("/srv/projects")
    );

    let commands = runtime.commands.lock().expect("commands lock").clone();
    assert_eq!(
        commands,
        vec![
            (
                "ws-remote".to_string(),
                "/srv/projects/openbitfun".to_string(),
                vec!["rev-parse".to_string(), "--show-toplevel".to_string()],
            ),
            (
                "ws-remote".to_string(),
                "/srv/projects".to_string(),
                vec!["remote".to_string(), "-v".to_string()],
            ),
        ],
        "git probes must run through the remote runtime, keyed by the workspace ID"
    );
}

#[tokio::test]
async fn repository_trusts_provider_identity_uses_remote_runtime() {
    let path = temp_token_store_path("remote-trust");
    let runtime = Arc::new(RecordingRemoteGitRuntime::default());
    let service = ReviewPlatformService::new(path, runtime);

    assert!(
        service
            .repository_trusts_provider_identity(
                &ReviewRepositoryLocator::new(
                    Some("ws-remote".to_string()),
                    "/srv/projects/openbitfun",
                ),
                ReviewPlatformKind::Gitlab,
                "gitlab.com",
                "example/repo",
            )
            .await,
        "remote workspace remotes must participate in provider identity trust"
    );
}

#[tokio::test]
async fn workspace_context_discovers_repository_without_listing_pull_requests() {
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
            "https://gitlab.com/example/repo.git",
        ],
    )
    .await
    .expect("GitLab remote should be added");
    let path = temp_token_store_path("workspace-context");
    let service = ReviewPlatformService::new_local_only(path.clone());

    let snapshot = service
        .workspace_context(&ReviewRepositoryLocator::legacy_path(repository_path), None)
        .await
        .expect("workspace context should not call the provider list API");

    assert!(snapshot.pull_requests.is_empty());
    assert_eq!(snapshot.pagination.total, Some(0));
    assert_eq!(
        snapshot
            .repository
            .as_ref()
            .map(|repository| repository.project_path.as_str()),
        Some("example/repo")
    );
    let _ = fs::remove_file(path).await;
}

#[tokio::test]
async fn repository_root_accepts_nested_and_file_paths() {
    let root = std::env::temp_dir().join(format!(
        "openbitfun-review-platform-git-root-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos()
    ));
    let nested = root.join("nested").join("dir");
    let file = nested.join("tracked.txt");
    fs::create_dir_all(&nested)
        .await
        .expect("temporary nested directory should be created");
    fs::write(&file, "content")
        .await
        .expect("temporary file should be created");
    // `git rev-parse --show-toplevel` reports the canonical path (for
    // example `/private/var/...` instead of `/var/...` on macOS).
    let root = root
        .canonicalize()
        .expect("temporary repository root should canonicalize");
    let nested = nested
        .canonicalize()
        .expect("nested directory should canonicalize");
    let file = nested.join("tracked.txt");

    execute_git_command(
        root.to_str()
            .expect("temporary repository path should be valid UTF-8"),
        &["init"],
    )
    .await
    .expect("git init should succeed for repository root test");

    let root_text = root
        .to_str()
        .expect("temporary repository path should be valid UTF-8");
    // Git for Windows reports a normal drive path, while
    // std::fs::canonicalize uses the equivalent verbatim path form.
    #[cfg(windows)]
    let root_text = root_text.strip_prefix(r"\\?\").unwrap_or(root_text);
    let expected = normalize_repository_root(root_text);
    let nested_root = get_repository_root(
        nested
            .to_str()
            .expect("nested repository path should be valid UTF-8"),
    )
    .await
    .expect("nested directory should resolve repository root");
    let file_root = get_repository_root(
        file.to_str()
            .expect("file repository path should be valid UTF-8"),
    )
    .await
    .expect("file path should resolve repository root");

    assert_eq!(nested_root, expected);
    assert_eq!(file_root, expected);

    let _ = fs::remove_dir_all(root).await;
}
