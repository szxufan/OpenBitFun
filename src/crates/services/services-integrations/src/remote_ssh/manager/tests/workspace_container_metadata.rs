use super::super::*;
#[cfg(unix)]
use super::support::shell_workspace_provider_fixture;

#[test]
fn parses_container_directory_entry_with_newline_and_unit_separator() {
    let entries = parse_container_dir_output(
        "src\n\u{1f}name\x00/workspace/src\n\u{1f}name\x00d\x00\x001720000000\x00755\x00",
    )
    .unwrap();
    let entry = &entries[0];

    assert_eq!(entry.name, "src\n\u{1f}name");
    assert_eq!(entry.path, "/workspace/src\n\u{1f}name");
    assert!(entry.is_dir);
    assert_eq!(entry.modified, Some(1_720_000_000_000));
    assert_eq!(entry.permissions.as_deref(), Some("755"));
}

#[test]
fn container_metadata_does_not_follow_symlinks_or_read_file_bodies() {
    let symlink_check = CONTAINER_ENTRY_METADATA_SCRIPT.find("[ -L").unwrap();
    let directory_check = CONTAINER_ENTRY_METADATA_SCRIPT.find("[ -d").unwrap();

    assert!(symlink_check < directory_check);
    assert!(CONTAINER_ENTRY_METADATA_SCRIPT.contains("[ -f"));
    assert!(CONTAINER_ENTRY_METADATA_SCRIPT.contains("else kind=o"));
    assert!(!CONTAINER_ENTRY_METADATA_SCRIPT.contains("wc -c"));
    assert!(CONTAINER_ENTRY_METADATA_SCRIPT.contains("stat -c %s"));
    assert!(CONTAINER_ENTRY_METADATA_SCRIPT.contains("stat -f %z"));
}

#[test]
fn bounded_container_directory_script_stops_the_nul_producer() {
    let bounded = container_read_dir_script("/workspace", Some(4096));
    let unbounded = container_read_dir_script("/workspace", None);

    assert!(bounded.contains("find -H \"$dir\" -mindepth 1 -maxdepth 1 -print0"));
    assert!(bounded.contains("head -z -n 4096"));
    assert!(bounded.contains("xargs -0 -n 64"));
    assert!(!bounded.contains("\"$dir\"/*"));
    assert!(!unbounded.contains("head -z -n 4096"));
}

#[test]
fn container_special_files_are_not_reported_as_regular_files() {
    let entry =
        parse_container_file_output("pipe\x00/workspace/pipe\x00o\x000\x001720000000\x00644\x00")
            .unwrap()
            .unwrap();

    assert!(!entry.is_file);
    assert!(!entry.is_dir);
    assert!(!entry.is_symlink);
}

#[test]
fn container_exists_retains_transport_and_command_errors() {
    assert!(parse_container_exists_output("/workspace/file", b"exists", b"", 0).unwrap());
    assert!(!parse_container_exists_output("/workspace/missing", b"missing", b"", 0).unwrap());
    for (status, stderr) in [
        (1, "Error response from daemon: No such container"),
        (1, "wsl.exe: WSL2 is not installed"),
        (2, "Permission denied"),
        (125, "Cannot connect to the Docker daemon"),
        (126, "Shell is not executable"),
        (127, ""),
    ] {
        let error =
            parse_container_exists_output("/workspace/file", b"", stderr.as_bytes(), status)
                .expect_err("container failures must not be treated as missing files")
                .to_string();
        assert!(error.contains(&format!("status {status}")));
        assert!(error.contains(stderr));
    }
    assert!(parse_container_exists_output("/workspace/missing", b"", b"", 1).is_err());
    assert!(parse_container_exists_output("/workspace/missing", b"", b"", 0).is_err());
    assert!(parse_container_exists_output("/workspace/missing", b"unknown", b"", 0).is_err());
}

/// `wsl.exe` prints this host warning on every launch, including launches
/// whose Linux command succeeds. It arrives as UTF-16LE bytes on the same
/// stderr stream as the Linux command.
fn wsl_host_warning_stderr() -> Vec<u8> {
    "wsl: 检测到 localhost 代理配置，但未镜像到 WSL。NAT 模式下的 WSL 不支持 localhost 代理。\r\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn container_dir_listing() -> Vec<u8> {
    format!(
        "{}\u{0}",
        [
            "AGENTS.md",
            "/workspace/AGENTS.md",
            "f",
            "15",
            "1720000000",
            "644"
        ]
        .join("\u{0}")
    )
    .into_bytes()
}

#[test]
fn container_directory_listing_succeeds_with_host_warnings_on_stderr() {
    let entries = container_dir_entries(
        "/workspace",
        container_dir_listing(),
        &wsl_host_warning_stderr(),
        0,
    )
    .expect("a successful listing must not fail because the transport wrote to stderr");

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "AGENTS.md");
    assert!(entries[0].is_file);
}

#[test]
fn container_directory_listing_reports_linux_errors_without_host_noise() {
    let mut stderr = wsl_host_warning_stderr();
    stderr.extend_from_slice(b"Container path is not a directory or is inaccessible\n");

    let error = container_dir_entries("/workspace", Vec::new(), &stderr, 44)
        .expect_err("a failing probe must not report a listing")
        .to_string();

    assert!(error.contains("status 44"));
    assert!(error.contains("Container path is not a directory or is inaccessible"));
    assert!(!error.contains("localhost"));
}

#[test]
fn container_exists_ignores_host_warnings() {
    let stderr = wsl_host_warning_stderr();
    assert!(parse_container_exists_output("/workspace/file", b"exists", &stderr, 0).unwrap());
    assert!(!parse_container_exists_output("/workspace/missing", b"missing", &stderr, 0).unwrap());
}

#[test]
fn container_failure_details_keep_linux_text_and_drop_host_noise() {
    let mut stderr = wsl_host_warning_stderr();
    stderr.extend_from_slice(b"cat: /workspace/missing: No such file or directory\n");

    assert_eq!(
        container_stderr_detail(&stderr),
        "cat: /workspace/missing: No such file or directory"
    );
    assert_eq!(container_stderr_detail(&[]), "no stderr output");
    assert_eq!(
        container_stderr_detail(&wsl_host_warning_stderr()),
        "wsl: 检测到 localhost 代理配置，但未镜像到 WSL。NAT 模式下的 WSL 不支持 localhost 代理。"
    );
}

#[test]
fn workspace_container_failure_retains_host_launch_error() {
    for stderr in [
        b"wsl.exe: WSL2 is not installed\r\n".to_vec(),
        "wsl: Failed to start distribution\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect(),
    ] {
        let detail = container_stderr_detail(&stderr);
        assert!(detail.contains("WSL2 is not installed") || detail.contains("Failed to start"));
        let error = parse_container_exists_output("/workspace/file", b"", &stderr, 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&detail));
    }
}

#[test]
fn workspace_container_listing_rejects_producer_errors_with_zero_exit_status() {
    for stdout in [Vec::new(), container_dir_listing()] {
        let mut stderr = wsl_host_warning_stderr();
        stderr.extend_from_slice(b"find: /workspace: Permission denied\n");
        let error = container_dir_entries("/workspace", stdout, &stderr, 0)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Permission denied"));
        assert!(!error.contains("localhost"));
    }
}

#[test]
fn workspace_container_listing_ignores_consecutive_utf16_warnings() {
    let stderr = "wsl: 上次启动警告\r\nwsl.exe: second warning\r\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let entries = container_dir_entries("/workspace", container_dir_listing(), &stderr, 0)
        .expect("UTF-16 host messages must not be mistaken for Linux errors");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "AGENTS.md");
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_container_unreadable_directory_is_not_empty() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let (manager, _) = shell_workspace_provider_fixture(temp.path()).await;
    let denied = temp.path().join("denied");
    std::fs::create_dir(&denied).unwrap();
    std::fs::write(denied.join("existing.txt"), b"exists").unwrap();
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0)).unwrap();
    let result = manager
        .container_read_dir("workspace-shell-fixture", denied.to_str().unwrap())
        .await;
    // Restore before asserting so temporary data remains removable on failure.
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Privileged CI hosts can read a mode-000 directory. The producer-error
    // test above covers the parser independently of host privileges.
    match result {
        Err(error) => assert!(error.to_string().contains("Permission denied")),
        Ok(entries) => assert!(entries.iter().any(|entry| entry.name == "existing.txt")),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_container_exists_requires_a_probe_response() {
    let temp = tempfile::tempdir().unwrap();
    let (manager, _) = shell_workspace_provider_fixture(temp.path()).await;
    let existing = temp.path().join("existing file");
    let missing = temp.path().join("missing file");
    std::fs::write(&existing, b"exists").unwrap();
    assert!(manager
        .container_exists("workspace-shell-fixture", existing.to_str().unwrap())
        .await
        .unwrap());
    assert!(!manager
        .container_exists("workspace-shell-fixture", missing.to_str().unwrap())
        .await
        .unwrap());
    let executable = temp.path().join("docker shell fixture");
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf 'wsl.exe: WSL2 is not installed\\n' >&2\nexit 1\n",
    )
    .unwrap();
    let error = manager
        .container_exists("workspace-shell-fixture", existing.to_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("WSL2 is not installed"));
}
