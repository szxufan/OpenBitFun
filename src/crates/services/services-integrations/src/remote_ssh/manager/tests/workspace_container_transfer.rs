#[cfg(unix)]
use super::{super::*, support::shell_workspace_provider_fixture};

#[cfg(unix)]
#[tokio::test]
async fn workspace_container_write_preserves_inode_links_and_modes_without_changing_uploads() {
    use openbitfun_runtime_ports::WorkspaceFileSystem;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    let (manager, provider) = shell_workspace_provider_fixture(temp.path()).await;
    let target = temp.path().join("target");
    let link = temp.path().join("link ' with\\name\nline");
    let hardlink = temp.path().join("hardlink");
    std::fs::write(&target, b"original").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o751)).unwrap();
    symlink(&target, &link).unwrap();
    std::fs::hard_link(&target, &hardlink).unwrap();
    let before = std::fs::metadata(&target).unwrap();
    provider
        .write_file(link.to_str().unwrap(), b"replacement\0bytes")
        .await
        .unwrap();
    let after = std::fs::metadata(&target).unwrap();
    assert_eq!(before.ino(), after.ino());
    assert_eq!(after.permissions().mode() & 0o7777, 0o751);
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read(&target).unwrap(), b"replacement\0bytes");
    assert_eq!(std::fs::read(&hardlink).unwrap(), b"replacement\0bytes");
    provider
        .write_file(target.to_str().unwrap(), b"")
        .await
        .unwrap();
    assert_eq!(std::fs::metadata(&target).unwrap().ino(), before.ino());
    assert!(std::fs::read(&target).unwrap().is_empty());
    let created = temp.path().join("nested/new file");
    provider
        .write_file(created.to_str().unwrap(), b"new file")
        .await
        .unwrap();
    assert_eq!(std::fs::read(&created).unwrap(), b"new file");

    // The independent upload contract continues replacing its destination.
    manager
        .container_write_file(
            "workspace-shell-fixture",
            link.to_str().unwrap(),
            b"uploaded",
        )
        .await
        .unwrap();
    assert!(!std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read(&link).unwrap(), b"uploaded");
    assert!(std::fs::read(&target).unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_container_write_rejects_incomplete_transfer_before_touching_target() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let staged = temp.path().join(".openbitfun-upload-test.tmp");
    std::fs::write(&target, b"preserved").unwrap();
    let command =
        container_workspace_write_command(target.to_str().unwrap(), staged.to_str().unwrap(), 100);
    let transport =
        crate::remote_ssh::WorkspaceStdio::spawn_local_process("sh", &["-c".into(), command])
            .unwrap();
    let (mut stdin, mut stdout, mut stderr, _control, completion) = transport.into_parts();
    stdin.write_all(b"incomplete").await.unwrap();
    stdin.shutdown().await.unwrap();
    let mut output = Vec::new();
    let mut diagnostics = String::new();
    stdout.read_to_end(&mut output).await.unwrap();
    stderr.read_to_string(&mut diagnostics).await.unwrap();
    assert_eq!(completion.wait().await.exit_code, Some(65));
    assert!(diagnostics.contains("incomplete"));
    assert_eq!(std::fs::read(&target).unwrap(), b"preserved");
    assert!(!staged.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_container_streams_a_local_file_and_keeps_the_destination_on_cancel() {
    let temp = tempfile::tempdir().unwrap();
    let (manager, _provider) = shell_workspace_provider_fixture(temp.path()).await;
    let source = temp.path().join("source file.bin");
    let target = temp.path().join("target.bin");
    let payload = vec![7_u8; 700_000];
    std::fs::write(&source, &payload).unwrap();
    std::fs::write(&target, b"preserved").unwrap();

    let mut progress = Vec::new();
    let written = manager
        .container_write_file_from_path_with_progress(
            "workspace-shell-fixture",
            target.to_str().unwrap(),
            &source,
            &mut |written, total| {
                progress.push((written, total));
                true
            },
        )
        .await
        .unwrap();
    assert_eq!(written, payload.len() as u64);
    assert_eq!(
        std::fs::read(&target).unwrap(),
        payload,
        "a completed stream must replace the destination with the local file"
    );
    assert_eq!(progress.first(), Some(&(0, payload.len() as u64)));
    assert_eq!(
        progress.last(),
        Some(&(payload.len() as u64, payload.len() as u64))
    );
    assert!(progress.windows(2).all(|pair| pair[0].0 <= pair[1].0));

    // The staging command publishes through a rename, so a cancelled
    // stream must leave the previously committed destination untouched.
    let cancelled = manager
        .container_write_file_from_path_with_progress(
            "workspace-shell-fixture",
            target.to_str().unwrap(),
            &source,
            &mut |written, _| written < 262_144,
        )
        .await;
    assert!(cancelled.is_err());
    assert_eq!(std::fs::read(&target).unwrap(), payload);
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_container_write_stops_after_signal_between_staging_and_commit() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let staged = temp.path().join(".openbitfun-upload-test.tmp");
    let wc = temp.path().join("wc");
    std::fs::write(&target, b"preserved").unwrap();
    // wc is called only after staging is complete. Signal the writing
    // shell while it waits for this successful size-check subprocess;
    // the trap must exit instead of proceeding into target redirection.
    std::fs::write(
        &wc,
        "#!/bin/sh\ncat >/dev/null\nkill -TERM \"$OPENBITFUN_TEST_WRITE_PID\"\nprintf '8\\n'\n",
    )
    .unwrap();
    std::fs::set_permissions(&wc, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        temp.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let command = format!(
        "OPENBITFUN_TEST_WRITE_PID=$$; export OPENBITFUN_TEST_WRITE_PID; PATH={}; export PATH; {}",
        crate::remote_ssh::shell::quote_arg(&path),
        container_workspace_write_command(target.to_str().unwrap(), staged.to_str().unwrap(), 8)
    );
    let transport =
        crate::remote_ssh::WorkspaceStdio::spawn_local_process("sh", &["-c".into(), command])
            .unwrap();
    let (mut stdin, mut stdout, mut stderr, _control, completion) = transport.into_parts();
    stdin.write_all(b"modified").await.unwrap();
    stdin.shutdown().await.unwrap();
    let mut output = Vec::new();
    let mut diagnostics = Vec::new();
    stdout.read_to_end(&mut output).await.unwrap();
    stderr.read_to_end(&mut diagnostics).await.unwrap();
    assert_eq!(completion.wait().await.exit_code, Some(143));
    assert_eq!(std::fs::read(&target).unwrap(), b"preserved");
    assert!(!staged.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_container_reader_streams_bytes_and_checks_exit_status() {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let transport = crate::remote_ssh::WorkspaceStdio::spawn_local_process(
        "sh",
        &["-c".to_string(), "printf 'a\\000b'".to_string()],
    )
    .unwrap();
    let mut reader = ContainerFileReader::from_transport(transport);
    let seek_error = reader.seek(std::io::SeekFrom::Start(0)).await.unwrap_err();
    assert_eq!(seek_error.kind(), std::io::ErrorKind::Unsupported);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, b"a\0b");

    let transport = crate::remote_ssh::WorkspaceStdio::spawn_local_process(
        "sh",
        &[
            "-c".to_string(),
            "printf partial; printf denied >&2; exit 13".to_string(),
        ],
    )
    .unwrap();
    let mut reader = ContainerFileReader::from_transport(transport);
    let mut bytes = Vec::new();
    let error = reader.read_to_end(&mut bytes).await.unwrap_err();
    assert_eq!(bytes, b"partial");
    assert!(error.to_string().contains("Some(13)"), "{error}");
    assert!(error.to_string().contains("denied"), "{error}");
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_container_reader_drop_cancels_backpressured_child() {
    let transport = crate::remote_ssh::WorkspaceStdio::spawn_local_process(
        "sh",
        &[
            "-c".to_string(),
            "while :; do printf 'stream content\\n'; done".to_string(),
        ],
    )
    .unwrap();
    let reader = ContainerFileReader::from_transport(transport);
    let completion = reader.completion.clone();
    drop(reader);
    tokio::time::timeout(Duration::from_secs(5), completion.wait())
        .await
        .expect("dropping reader must release all streams and stop the child");
}
