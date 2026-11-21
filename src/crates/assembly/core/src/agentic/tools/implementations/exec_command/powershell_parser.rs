use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

/// Launch only the selected native parser with a fixed, profile-free helper.
/// User command text travels over stdin and is never interpolated into code.
pub(super) async fn parse(shell: &Path, source: &str) -> Result<String, String> {
    if source.len() > tool_runtime::shell_analysis::MAX_COMMAND_BYTES {
        return Err("PowerShell source limit".into());
    }
    let mut command = openbitfun_services_core::process_manager::create_tokio_command(shell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("powershell_ast.ps1"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| "PowerShell parser could not start")?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or("PowerShell parser stdin unavailable")?;
    let parse = async {
        stdin
            .write_all(source.as_bytes())
            .await
            .map_err(|_| "PowerShell parser input failed")?;
        drop(stdin);
        let output = child
            .wait_with_output()
            .await
            .map_err(|_| "PowerShell parser failed")?;
        if !output.status.success()
            || output.stdout.len() > tool_runtime::shell_analysis::MAX_COMMAND_BYTES
        {
            return Err("PowerShell AST unavailable or too large");
        }
        String::from_utf8(output.stdout).map_err(|_| "PowerShell parser output is not UTF-8")
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), parse)
        .await
        .map_err(|_| "PowerShell parser timed out".to_string())?
        .map_err(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires OPENBITFUN_TEST_POWERSHELL pointing to a trusted native PowerShell binary"]
    async fn command_permission_native_powershell_ast_parses_without_execution() {
        let shell = std::env::var_os("OPENBITFUN_TEST_POWERSHELL")
            .expect("explicit trusted PowerShell path");
        let shell = Path::new(&shell);
        let program = parse(
            shell,
            "git clone 'url with spaces'; Get-Content README.md > output.txt",
        )
        .await
        .unwrap();
        let facts = tool_runtime::shell_analysis::powershell_permission_program(&program, "/repo");
        assert!(facts.complete, "{program}");
        assert_eq!(facts.commands.len(), 2);
        assert_eq!(facts.commands[0].argv, ["git", "clone", "url with spaces"]);
        assert!(facts
            .accesses
            .contains(&("edit".into(), "/repo/output.txt".into())));
        for (source, expected) in [
            (
                "Set-Location ./sub; Get-Content file.txt",
                "/repo/sub/file.txt",
            ),
            ("cd ./sub && Get-Content file.txt", "/repo/sub/file.txt"),
            ("sl ./sub || Get-Content file.txt", "/repo/file.txt"),
            (
                "Push-Location ./sub; Pop-Location; Get-Content file.txt",
                "/repo/file.txt",
            ),
            (
                "Set-Location ./sub | Get-Content file.txt",
                "/repo/sub/file.txt",
            ),
            (
                "Push-Location ./sub && Pop-Location && Get-Content file.txt",
                "/repo/file.txt",
            ),
        ] {
            let program = parse(shell, source).await.unwrap();
            let facts =
                tool_runtime::shell_analysis::powershell_permission_program(&program, "/repo");
            // Semicolon can leave an uncertain stack when Push-Location fails.
            if source.starts_with("Push-Location ./sub;") {
                assert!(!facts.complete);
                continue;
            }
            assert!(facts.complete, "{source}: {program}");
            assert!(facts.commands.iter().all(|c| c.read_only), "{source}");
            assert!(
                facts.accesses.contains(&("read".into(), expected.into())),
                "{source}: {facts:?}"
            );
        }
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("never-created");
        let source = format!(
            "Get-Content $(New-Item -ItemType File -Path '{}')",
            marker.to_string_lossy().replace('\'', "''")
        );
        let program = parse(shell, &source).await.unwrap();
        assert!(
            !tool_runtime::shell_analysis::powershell_permission_program(&program, "/repo")
                .complete
        );
        assert!(!marker.exists());
        let program = parse(shell, "Get-Content $env:SECRET").await.unwrap();
        assert!(
            !tool_runtime::shell_analysis::powershell_permission_program(&program, "/repo")
                .complete
        );
    }
}
