use super::super::*;

fn proxy_command_fixture_config(command: &str) -> SSHConnectionConfig {
    SSHConnectionConfig {
        id: "proxy-fixture".to_string(),
        name: "proxy fixture".to_string(),
        host: "coder-vscode.coder.sage.org.ai--abc".to_string(),
        port: 22,
        username: "coder".to_string(),
        auth: SSHAuthMethod::PrivateKey {
            key_path: "/nonexistent/key".to_string(),
            passphrase: None,
            certificate_path: None,
        },
        default_workspace: None,
        proxy_jump: None,
        proxy_command: Some(command.to_string()),
        container: None,
        wsl: None,
        options: Default::default(),
    }
}

#[tokio::test]
async fn proxy_command_transport_reports_proxy_exit_code() {
    let temp = tempfile::tempdir().unwrap();
    let manager = SSHConnectionManager::new(temp.path().into());
    let config = proxy_command_fixture_config(if cfg!(windows) {
        "exit 7"
    } else {
        "sh -c 'exit 7'"
    });

    let error = manager
        .establish_session(&config, 10)
        .await
        .err()
        .expect("a proxy that exits immediately cannot complete a handshake");
    let message = error.to_string();
    assert!(
        message.contains("proxy command exited with code 7"),
        "unexpected error: {message}"
    );
}

#[tokio::test]
async fn proxy_command_rejects_jump_and_container_combinations() {
    let temp = tempfile::tempdir().unwrap();
    let manager = SSHConnectionManager::new(temp.path().into());

    let mut config = proxy_command_fixture_config("exit 0");
    config.proxy_jump = Some("jump.example".to_string());
    let error = manager
        .establish_session(&config, 10)
        .await
        .err()
        .expect("ProxyCommand plus ProxyJump is rejected in this release");
    assert!(
        error.to_string().contains("ProxyCommand and ProxyJump"),
        "unexpected error: {error}"
    );

    let mut config = proxy_command_fixture_config("exit 0");
    config.container = Some(ContainerWorkspaceConfig {
        name: "dev".to_string(),
        access: ContainerAccess::DockerExec,
        local: false,
        docker_path: "docker".to_string(),
        shell: "/bin/sh".to_string(),
        user: None,
        interactive: false,
    });
    let error = manager
        .establish_session(&config, 10)
        .await
        .err()
        .expect("ProxyCommand plus a container target is rejected in this release");
    assert!(
        error.to_string().contains("Docker container"),
        "unexpected error: {error}"
    );
}
