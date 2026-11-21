use super::super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
pub(super) async fn shell_workspace_provider_fixture(
    root: &std::path::Path,
) -> (SSHConnectionManager, crate::remote_ssh::RemoteWorkspaceFs) {
    use std::os::unix::fs::PermissionsExt;
    // Execute the actual Docker command adapter against a local POSIX shell.
    // This covers the shell/stream protocol, not an installed Docker daemon.
    let executable = root.join("docker shell fixture");
    std::fs::write(&executable, "#!/bin/sh\ncase \"$1\" in\ninspect) printf true;;\nexec) shift; [ \"$1\" != -i ] || shift; [ \"$1\" = fixture ] || exit 90; shift; exec \"$@\";;\n*) exit 91;;\nesac\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = SSHConnectionConfig {
        id: "workspace-shell-fixture".into(),
        name: "Workspace shell fixture".into(),
        host: String::new(),
        port: 22,
        username: String::new(),
        auth: SSHAuthMethod::Agent {
            key_fingerprint: None,
            fallback_key_path: None,
        },
        default_workspace: None,
        proxy_jump: None,
        proxy_command: None,
        container: Some(ContainerWorkspaceConfig {
            name: "fixture".into(),
            access: ContainerAccess::DockerExec,
            local: true,
            docker_path: executable.to_string_lossy().into_owned(),
            shell: "/bin/sh".into(),
            user: None,
            interactive: true,
        }),
        wsl: None,
        options: Default::default(),
    };
    let manager = SSHConnectionManager::new(root.join("manager-data"));
    manager.connections.write().await.insert(
        config.id.clone(),
        ActiveConnection {
            handle: None,
            jump_handles: Vec::new(),
            config: config.clone(),
            effective_config: config,
            server_info: None,
            sftp_session: Arc::new(SftpCache::new()),
            bounded_sftp_session: Arc::new(BoundedSftpCache::new()),
            server_key: None,
            proxy_child: None,
            alive: Arc::new(AtomicBool::new(true)),
            liveness: Arc::new(ConnectionLiveness::confirmed_now()),
        },
    );
    let file_service = crate::remote_ssh::RemoteFileService::new(Arc::new(
        tokio::sync::RwLock::new(Some(manager.clone())),
    ));
    let provider =
        crate::remote_ssh::RemoteWorkspaceFs::new("workspace-shell-fixture".into(), file_service);
    (manager, provider)
}

pub(super) fn test_data_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "openbitfun-remote-ssh-manager-{}-{}-{}",
        name,
        std::process::id(),
        nanos
    ))
}
