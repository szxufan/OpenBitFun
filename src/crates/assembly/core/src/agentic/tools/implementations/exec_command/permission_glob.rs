use crate::agentic::tools::framework::ToolUseContext;
use tool_runtime::shell_analysis::PermissionGlobMatcher;

/// Enumerate immediate children through the execution workspace's IO provider.
/// This never runs a shell or uses a controller-local mirror for remote paths.
pub(super) async fn expand(
    context: &ToolUseContext,
    patterns: &[String],
) -> Result<Vec<(String, String)>, &'static str> {
    let mut accesses = vec![];
    for pattern in patterns {
        let matcher = PermissionGlobMatcher::new(pattern).ok_or("unsupported command path glob")?;
        let (parent, _) = pattern
            .rsplit_once('/')
            .ok_or("glob directory is unavailable")?;
        let parent = if parent.is_empty() { "/" } else { parent };
        let candidates =
            crate::agentic::execution::edit_constraint_guard::inspect_shell_permission_path(
                context, parent,
            )
            .await?;
        let directory = &candidates[0];
        let resolved = context
            .resolve_tool_path(directory)
            .map_err(|_| "glob directory resolution failed")?;
        if context.is_remote() && !resolved.uses_remote_workspace_backend() {
            return Err("remote glob directory resolved to a local backend");
        }
        let fs = context
            .file_system_for_path(&resolved)
            .map_err(|_| "glob filesystem is unavailable")?;
        let entries = fs
            .read_dir_bounded(&resolved.resolved_path, 1025)
            .await
            .map_err(|_| "glob directory listing failed")?;
        if entries.len() > 1024 {
            return Err("command glob directory entry limit");
        }
        accesses.push(("read".into(), directory.clone()));
        for entry in entries {
            if !matcher.matches(&entry.name) {
                continue;
            }
            if entry.name.starts_with('-')
                || entry.name.contains(['/', '\\', '\0', '\u{fffd}'])
                || matches!(entry.name.as_str(), "." | "..")
            {
                return Err("glob filename cannot be represented safely");
            }
            // Include dotfiles conservatively, covering trusted shell dotglob settings.
            let path = format!("{}/{}", directory.trim_end_matches('/'), entry.name);
            accesses.push(("read".into(), path));
            if accesses.len() > 4096
                || accesses.iter().map(|(_, path)| path.len()).sum::<usize>() > 1024 * 1024
            {
                return Err("command glob access limit");
            }
        }
    }
    Ok(accesses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbitfun_runtime_ports::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct TargetFs {
        count: usize,
        reads: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl WorkspaceFileSystem for TargetFs {
        async fn read_file(&self, _: &str) -> anyhow::Result<Vec<u8>> {
            unreachable!("glob preparation reads metadata only")
        }
        async fn read_file_text(&self, _: &str) -> anyhow::Result<String> {
            unreachable!("glob preparation reads metadata only")
        }
        async fn write_file(&self, _: &str, _: &[u8]) -> anyhow::Result<()> {
            unreachable!("glob preparation must not write")
        }
        async fn exists(&self, _: &str) -> anyhow::Result<bool> {
            Ok(true)
        }
        async fn is_file(&self, _: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
        async fn is_dir(&self, _: &str) -> anyhow::Result<bool> {
            Ok(true)
        }
        async fn path_kind_no_follow(
            &self,
            path: &str,
        ) -> anyhow::Result<Option<WorkspacePathKind>> {
            assert!(matches!(
                path,
                "/target" | "/target/repo" | "/target/repo/games"
            ));
            Ok(Some(WorkspacePathKind::Directory))
        }
        async fn read_dir(&self, path: &str) -> anyhow::Result<Vec<WorkspaceDirEntry>> {
            assert_eq!(path, "/target/repo/games");
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok((0..self.count)
                .map(|index| WorkspaceDirEntry {
                    name: format!("game{index}.js"),
                    path: String::from("unused"),
                    is_dir: false,
                    is_symlink: false,
                    modified: None,
                })
                .collect())
        }
    }
    #[async_trait::async_trait]
    impl WorkspaceShell for TargetFs {
        async fn exec_with_options(
            &self,
            _: &str,
            _: WorkspaceCommandOptions,
        ) -> anyhow::Result<WorkspaceCommandResult> {
            unreachable!("glob preparation must not execute a shell")
        }
    }
    #[tokio::test]
    async fn command_permission_glob_uses_remote_provider_and_rejects_truncated_lists() {
        use crate::agentic::workspace::WorkspaceBinding;
        use crate::service::remote_ssh::workspace_state::workspace_session_identity;
        for count in [1, 1025] {
            let fs = Arc::new(TargetFs {
                count,
                reads: AtomicUsize::new(0),
            });
            let binding = WorkspaceBinding::new_remote(
                None,
                "/target/repo".into(),
                "conn".into(),
                "Target".into(),
                workspace_session_identity("/target/repo", Some("conn"), Some("target")).unwrap(),
            );
            let mut context = ToolUseContext::for_tool_listing(Some(binding), None);
            let patterns = vec!["/target/repo/games/*.js".into()];
            assert!(expand(&context, &patterns).await.is_err());
            context.runtime_handles = ToolRuntimeHandles::new(
                Some(WorkspaceServices {
                    fs: fs.clone(),
                    shell: fs.clone(),
                }),
                None,
            );
            let result = expand(&context, &patterns).await;
            if count == 1 {
                assert_eq!(
                    result.unwrap(),
                    [
                        ("read".into(), "/target/repo/games".into()),
                        ("read".into(), "/target/repo/games/game0.js".into())
                    ]
                );
            } else {
                assert!(result.is_err());
            }
            assert_eq!(fs.reads.load(Ordering::SeqCst), 1);
        }
    }
}
