use super::super::*;
#[cfg(unix)]
use super::support::shell_workspace_provider_fixture;

#[cfg(unix)]
#[tokio::test]
async fn workspace_recursive_directory_delete_shell() {
    use openbitfun_runtime_ports::WorkspaceFileSystem;
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let (_manager, provider) = shell_workspace_provider_fixture(temp.path()).await;
    let target = temp.path().join("delete 'quoted' $(printf injected) ");
    let keep = temp.path().join("keep");
    std::fs::write(&keep, b"keep").unwrap();
    std::fs::create_dir_all(target.join("nested")).unwrap();
    for file in 0..100 {
        std::fs::write(target.join("nested").join(file.to_string()), b"delete").unwrap();
    }
    symlink(&keep, target.join("outside-link")).unwrap();
    assert!(provider
        .remove_dir(target.to_str().unwrap(), false)
        .await
        .is_err());
    assert!(target.join("nested/0").exists());
    provider
        .remove_dir(&format!("{}/", target.display()), true)
        .await
        .unwrap();
    assert!(!target.exists());
    assert_eq!(std::fs::read(&keep).unwrap(), b"keep");
    let error = provider
        .remove_dir(keep.to_str().unwrap(), true)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not a directory"));
    assert_eq!(std::fs::read(&keep).unwrap(), b"keep");
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_directory_symlink_browsing_shell() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let (manager, _provider) = shell_workspace_provider_fixture(temp.path()).await;
    let service = crate::remote_ssh::RemoteFileService::new(Arc::new(tokio::sync::RwLock::new(
        Some(manager),
    )));
    let connection = "workspace-shell-fixture";
    let directory = temp.path().join("target directory");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("sentinel.txt"), b"keep").unwrap();
    symlink("..", directory.join("parent-link")).unwrap();
    symlink(&directory, temp.path().join("absolute-link")).unwrap();
    symlink("target directory", temp.path().join("relative-link")).unwrap();
    symlink("relative-link", temp.path().join("chained-link")).unwrap();
    symlink("missing", temp.path().join("broken-link")).unwrap();
    symlink(
        "target directory/sentinel.txt",
        temp.path().join("file-link"),
    )
    .unwrap();
    symlink("loop-link", temp.path().join("loop-link")).unwrap();

    let entries = service
        .read_dir(connection, temp.path().to_str().unwrap())
        .await
        .unwrap();
    for name in ["absolute-link", "relative-link", "chained-link"] {
        let entry = entries.iter().find(|entry| entry.name == name).unwrap();
        assert!(entry.is_symlink);
        assert!(!entry.is_dir, "listing must retain no-follow metadata");
        assert!(service.is_dir(connection, &entry.path).await.unwrap());
        let children = service.read_dir(connection, &entry.path).await.unwrap();
        assert_eq!(children.len(), 2);
        let file = children
            .iter()
            .find(|child| child.name == "sentinel.txt")
            .unwrap();
        assert_eq!(file.path, format!("{}/sentinel.txt", entry.path));
        let link = children
            .iter()
            .find(|child| child.name == "parent-link")
            .unwrap();
        assert!(link.is_symlink && !link.is_dir);
    }
    for name in ["file-link", "broken-link"] {
        let path = temp.path().join(name);
        assert!(!service
            .is_dir(connection, path.to_str().unwrap())
            .await
            .unwrap());
        assert!(service
            .read_dir(connection, path.to_str().unwrap())
            .await
            .is_err());
    }
    assert!(service
        .is_dir(connection, temp.path().join("loop-link").to_str().unwrap())
        .await
        .is_err());
    service
        .remove_dir_all(
            connection,
            temp.path().join("relative-link").to_str().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(directory.join("sentinel.txt")).unwrap(),
        b"keep"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_container_metadata_preserves_errors_and_follows_links() {
    use openbitfun_runtime_ports::{WorkspaceFileSystem, WorkspacePathKind};
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    let (_manager, provider) = shell_workspace_provider_fixture(temp.path()).await;
    let target = temp.path().join("target");
    let file_link = temp.path().join("file-link");
    let directory = temp.path().join("directory");
    let directory_link = temp.path().join("directory-link");
    let dangling = temp.path().join("dangling");
    std::fs::write(&target, b"target").unwrap();
    std::fs::create_dir(&directory).unwrap();
    symlink(&target, &file_link).unwrap();
    symlink(&directory, &directory_link).unwrap();
    symlink(temp.path().join("missing"), &dangling).unwrap();
    assert!(provider.is_file(file_link.to_str().unwrap()).await.unwrap());
    assert!(provider
        .is_dir(directory_link.to_str().unwrap())
        .await
        .unwrap());
    assert_eq!(
        provider
            .path_kind_no_follow(file_link.to_str().unwrap())
            .await
            .unwrap(),
        Some(WorkspacePathKind::Symlink)
    );
    assert_eq!(
        provider
            .path_kind_no_follow(dangling.to_str().unwrap())
            .await
            .unwrap(),
        Some(WorkspacePathKind::Symlink)
    );
    assert!(!provider.exists(dangling.to_str().unwrap()).await.unwrap());
    // readlink emits its own final newline. Removing it must preserve all
    // whitespace that belongs to the link target, including trailing LF.
    let unusual_name = "missing ' target\\name \n\n";
    std::fs::write(temp.path().join(unusual_name.trim_end()), b"wrong target").unwrap();
    std::fs::write(
        temp.path().join(unusual_name.trim_end_matches('\n')),
        b"wrong newline target",
    )
    .unwrap();
    let unusual_link = temp.path().join("unusual-target-link");
    symlink(unusual_name, &unusual_link).unwrap();
    let chained_link = temp.path().join("chained-link");
    symlink("unusual-target-link", &chained_link).unwrap();
    assert!(!provider
        .exists(unusual_link.to_str().unwrap())
        .await
        .unwrap());
    assert!(!provider
        .exists(chained_link.to_str().unwrap())
        .await
        .unwrap());
    let loop_link = temp.path().join("loop-link");
    symlink(&loop_link, &loop_link).unwrap();
    assert!(provider.exists(loop_link.to_str().unwrap()).await.is_err());
    assert_eq!(
        provider
            .path_kind_no_follow(loop_link.to_str().unwrap())
            .await
            .unwrap(),
        Some(WorkspacePathKind::Symlink)
    );
    assert!(!provider
        .exists(temp.path().join("missing").to_str().unwrap())
        .await
        .unwrap());
    let invalid_parent = target.join("child").to_string_lossy().into_owned();
    assert!(provider.exists(&invalid_parent).await.is_err());
    assert!(provider.is_file(&invalid_parent).await.is_err());
    assert!(provider.is_dir(&invalid_parent).await.is_err());
    assert!(provider.path_kind_no_follow(&invalid_parent).await.is_err());

    let denied = directory.join("hidden");
    let denied_link = temp.path().join("denied-link");
    std::fs::write(&denied, b"hidden").unwrap();
    symlink(&denied, &denied_link).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root CI runners can bypass POSIX mode checks; the invalid-parent
    // assertions above still require error preservation on every runner.
    let access_denied = std::fs::metadata(&denied).is_err();
    let exists = provider.exists(denied.to_str().unwrap()).await;
    let kind = provider.path_kind_no_follow(denied.to_str().unwrap()).await;
    let linked_exists = provider.exists(denied_link.to_str().unwrap()).await;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    if access_denied {
        assert!(
            exists.is_err(),
            "permission failures must not become absent files"
        );
        assert!(
            kind.is_err(),
            "no-follow metadata must preserve permission failures"
        );
        assert!(
            linked_exists.is_err(),
            "a link to an inaccessible target must not look absent"
        );
    }
}

#[test]
fn workspace_metadata_parses_permissions_and_rejects_malformed_stat() {
    use openbitfun_runtime_ports::WorkspacePathKind;
    let gnu = parse_workspace_stat_output("hex 81a4 7 1700000000 644\n").unwrap();
    let bsd = parse_workspace_stat_output("oct 100644 7 1700000000 644\n").unwrap();
    assert_eq!(gnu, bsd);
    assert_eq!(gnu.kind, WorkspacePathKind::File);
    assert_eq!(gnu.size, Some(7));
    assert_eq!(gnu.permissions, Some(0o644));
    assert!(gnu.modified.is_some());
    assert_eq!(
        parse_workspace_stat_output("oct 120777 7 -1 777")
            .unwrap()
            .kind,
        WorkspacePathKind::Symlink
    );
    assert!(parse_workspace_stat_output("hex 81a4 7 unknown 644").is_err());
    assert!(parse_workspace_stat_output("").is_err());
}

#[test]
fn mkdir_all_prefixes_expand_absolute_posix_path() {
    assert_eq!(
        sftp_mkdir_all_prefixes("/home/wgq/workspace/bot_detection/.openbitfun/bin"),
        vec![
            "/home".to_string(),
            "/home/wgq".to_string(),
            "/home/wgq/workspace".to_string(),
            "/home/wgq/workspace/bot_detection".to_string(),
            "/home/wgq/workspace/bot_detection/.openbitfun".to_string(),
            "/home/wgq/workspace/bot_detection/.openbitfun/bin".to_string(),
        ]
    );
}

#[test]
fn mkdir_all_prefixes_collapse_redundant_separators() {
    assert_eq!(
        sftp_mkdir_all_prefixes("/home//wgq///project/"),
        vec![
            "/home".to_string(),
            "/home/wgq".to_string(),
            "/home/wgq/project".to_string(),
        ]
    );
}
