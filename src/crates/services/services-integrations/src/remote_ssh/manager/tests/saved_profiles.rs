use super::super::*;
use super::support::test_data_dir;
use crate::remote_ssh::types::RemoteWorkspace;

#[tokio::test]
async fn retains_password_connection_and_workspace_without_vault_entry() {
    let dir = test_data_dir("missing-vault-entry");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());

    tokio::fs::write(
        dir.join("ssh_connections.json"),
        serde_json::to_string_pretty(&serde_json::json!([{
            "id": "ssh-root@example.com",
            "name": "root@example.com",
            "host": "example.com",
            "port": 22,
            "username": "root",
            "authType": { "type": "Password" },
            "defaultWorkspace": "/root/project",
            "lastConnected": 1
        }]))
        .unwrap(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        dir.join("remote_workspace.json"),
        serde_json::to_string_pretty(&serde_json::json!([{
            "connectionId": "ssh-root@example.com",
            "remotePath": "/root/project",
            "connectionName": "root@example.com",
            "sshHost": "example.com"
        }]))
        .unwrap(),
    )
    .await
    .unwrap();

    manager.load_saved_connections().await.unwrap();
    manager.load_remote_workspace().await.unwrap();
    let removed = manager
        .prune_remote_workspaces_without_saved_connections()
        .await
        .unwrap();

    let saved = manager.get_saved_connections().await;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, "ssh-root@example.com");
    assert_eq!(saved[0].default_workspace.as_deref(), Some("/root/project"));
    assert!(removed.is_empty());
    let workspaces = manager.get_remote_workspaces().await;
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].connection_id, "ssh-root@example.com");
    assert_eq!(workspaces[0].remote_path, "/root/project");
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn rejects_single_remote_workspace_object() {
    let dir = test_data_dir("single-remote-workspace-object");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());

    tokio::fs::write(
        dir.join("remote_workspace.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "connectionId": "ssh-root@example.com",
            "remotePath": "/root/project",
            "connectionName": "root@example.com",
            "sshHost": "example.com"
        }))
        .unwrap(),
    )
    .await
    .unwrap();

    let error = manager.load_remote_workspace().await.unwrap_err();
    assert!(error
        .to_string()
        .contains("Failed to parse remote workspaces"));
    assert!(manager.get_remote_workspaces().await.is_empty());
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn rejects_saving_password_connection_without_password() {
    let dir = test_data_dir("empty-password-save");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());

    let result = manager
        .save_connection(&SSHConnectionConfig {
            id: "ssh-root@example.com:22".to_string(),
            name: "root@example.com".to_string(),
            host: "example.com".to_string(),
            port: 22,
            username: "root".to_string(),
            auth: SSHAuthMethod::Password {
                password: String::new(),
            },
            default_workspace: None,
            proxy_jump: None,
            proxy_command: None,
            container: None,
            wsl: None,
            options: Default::default(),
        })
        .await;

    assert!(result.is_err());
    assert!(manager.get_saved_connections().await.is_empty());
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn restores_connection_config_from_saved_password_profile() {
    let dir = test_data_dir("restore-password-config");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());

    manager
        .save_connection(&SSHConnectionConfig {
            id: "ssh-root@example.com:22".to_string(),
            name: "root@example.com".to_string(),
            host: "example.com".to_string(),
            port: 22,
            username: "root".to_string(),
            auth: SSHAuthMethod::Password {
                password: "secret".to_string(),
            },
            default_workspace: Some("/root/project".to_string()),
            proxy_jump: Some("jump-a,jump-b".to_string()),
            proxy_command: None,
            container: None,
            wsl: None,
            options: Default::default(),
        })
        .await
        .unwrap();

    let restored = manager
        .load_connection_config_from_saved("ssh-root@example.com:22")
        .await
        .unwrap()
        .expect("expected saved config");

    assert_eq!(restored.host, "example.com");
    assert_eq!(restored.username, "root");
    assert_eq!(restored.default_workspace.as_deref(), Some("/root/project"));
    assert_eq!(restored.proxy_jump.as_deref(), Some("jump-a,jump-b"));
    match restored.auth {
        SSHAuthMethod::Password { password } => assert_eq!(password, "secret"),
        other => panic!("expected password auth, got {:?}", other),
    }

    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn restores_legacy_local_docker_profile_without_password_vault_entry() {
    let dir = test_data_dir("legacy-local-docker-password-placeholder");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());

    tokio::fs::write(
        dir.join("ssh_connections.json"),
        serde_json::to_string_pretty(&serde_json::json!([{
            "id": "docker-local-legacy",
            "name": "local container",
            "host": "",
            "port": 22,
            "username": "",
            "authType": { "type": "Password" },
            "defaultWorkspace": "/workspace",
            "lastConnected": 1,
            "container": {
                "name": "dev",
                "access": "docker-exec",
                "local": true
            }
        }]))
        .unwrap(),
    )
    .await
    .unwrap();

    manager.load_saved_connections().await.unwrap();
    let restored = manager
        .load_connection_config_from_saved("docker-local-legacy")
        .await
        .unwrap()
        .expect("legacy local Docker profile must remain restorable");

    assert!(restored.uses_local_docker());
    assert!(matches!(
        restored.auth,
        SSHAuthMethod::Password { ref password } if password.is_empty()
    ));
    assert_eq!(
        restored.options,
        crate::remote_ssh::types::SSHConnectionOptions::default()
    );
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn saved_interactive_profile_never_persists_challenge_responses() {
    let dir = test_data_dir("interactive-secrets");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());

    manager
        .save_connection(&SSHConnectionConfig {
            id: "ssh-alice@example.com".to_string(),
            name: "interactive".to_string(),
            host: "example.com".to_string(),
            port: 22,
            username: "alice".to_string(),
            auth: SSHAuthMethod::KeyboardInteractive {
                responses: vec!["password-secret".to_string(), "123456".to_string()],
            },
            default_workspace: Some("/workspace".to_string()),
            proxy_jump: Some("jump.example.com".to_string()),
            proxy_command: None,
            container: None,
            wsl: None,
            options: Default::default(),
        })
        .await
        .unwrap();

    let persisted = tokio::fs::read_to_string(dir.join("ssh_connections.json"))
        .await
        .unwrap();
    assert!(persisted.contains("KeyboardInteractive"));
    assert!(!persisted.contains("password-secret"));
    assert!(!persisted.contains("123456"));

    let restored = manager
        .load_connection_config_from_saved("ssh-alice@example.com")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        restored.auth,
        SSHAuthMethod::KeyboardInteractive { ref responses } if responses.is_empty()
    ));
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn saved_agent_profile_preserves_identity_selection() {
    let dir = test_data_dir("agent-profile");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());

    manager
        .save_connection(&SSHConnectionConfig {
            id: "ssh-agent@example.com".to_string(),
            name: "agent".to_string(),
            host: "example.com".to_string(),
            port: 22,
            username: "agent".to_string(),
            auth: SSHAuthMethod::Agent {
                key_fingerprint: Some("SHA256:test-fingerprint".to_string()),
                fallback_key_path: Some("~/.ssh/custom-fallback".to_string()),
            },
            default_workspace: None,
            proxy_jump: None,
            proxy_command: None,
            container: None,
            wsl: None,
            options: Default::default(),
        })
        .await
        .unwrap();

    let restored = manager
        .load_connection_config_from_saved("ssh-agent@example.com")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        restored.auth,
        SSHAuthMethod::Agent {
            ref key_fingerprint,
            ref fallback_key_path,
        } if key_fingerprint.as_deref() == Some("SHA256:test-fingerprint")
            && fallback_key_path.as_deref() == Some("~/.ssh/custom-fallback")
    ));
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

fn key_profile(id: &str, host: &str, port: u16, username: &str) -> SSHConnectionConfig {
    SSHConnectionConfig {
        id: id.to_string(),
        name: format!("{username}@{host}:{port}"),
        host: host.to_string(),
        port,
        username: username.to_string(),
        auth: SSHAuthMethod::PrivateKey {
            key_path: "~/.ssh/id_ed25519".to_string(),
            passphrase: None,
            certificate_path: None,
        },
        default_workspace: None,
        proxy_jump: None,
        proxy_command: None,
        container: None,
        wsl: None,
        options: Default::default(),
    }
}

#[tokio::test]
async fn saving_a_second_port_on_the_same_host_keeps_both_profiles() {
    let dir = test_data_dir("port-scoped-profiles");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());

    manager
        .save_connection(&key_profile("ssh-root@cluster", "cluster", 22, "root"))
        .await
        .unwrap();
    manager
        .save_connection(&key_profile(
            "ssh-root@cluster#2201",
            "cluster",
            2201,
            "root",
        ))
        .await
        .unwrap();
    manager
        .save_connection(&key_profile(
            "ssh-root@cluster#2202",
            "cluster",
            2202,
            "root",
        ))
        .await
        .unwrap();

    let mut saved: Vec<(String, u16)> = manager
        .get_saved_connections()
        .await
        .into_iter()
        .map(|c| (c.id, c.port))
        .collect();
    saved.sort();
    assert_eq!(
        saved,
        vec![
            ("ssh-root@cluster".to_string(), 22),
            ("ssh-root@cluster#2201".to_string(), 2201),
            ("ssh-root@cluster#2202".to_string(), 2202),
        ]
    );

    // A legacy `:port` alias of the same endpoint is replaced; a distinct
    // id for the same endpoint is kept because records may reference it.
    manager
        .save_connection(&key_profile("ssh-root@cluster:22", "cluster", 22, "root"))
        .await
        .unwrap();
    manager
        .save_connection(&key_profile("ssh-root@cluster", "cluster", 22, "root"))
        .await
        .unwrap();
    manager
        .save_connection(&key_profile(
            "ssh-root@cluster#2202~2",
            "cluster",
            2202,
            "root",
        ))
        .await
        .unwrap();
    let mut ids: Vec<String> = manager
        .get_saved_connections()
        .await
        .into_iter()
        .map(|c| c.id)
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            "ssh-root@cluster".to_string(),
            "ssh-root@cluster#2201".to_string(),
            "ssh-root@cluster#2202".to_string(),
            "ssh-root@cluster#2202~2".to_string(),
        ]
    );
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn editing_the_port_of_a_saved_profile_keeps_its_id() {
    let dir = test_data_dir("port-edit-keeps-id");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());

    manager
        .save_connection(&key_profile(
            "ssh-root@cluster#2201",
            "cluster",
            2201,
            "root",
        ))
        .await
        .unwrap();
    manager
        .save_connection(&key_profile(
            "ssh-root@cluster#2201",
            "cluster",
            2299,
            "root",
        ))
        .await
        .unwrap();

    let saved = manager.get_saved_connections().await;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, "ssh-root@cluster#2201");
    assert_eq!(saved[0].port, 2299);
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn legacy_port_profiles_round_trip_unchanged_next_to_port_scoped_ones() {
    let dir = test_data_dir("legacy-port-profiles");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    // Shapes written by released builds: a post-#710 port-less id that
    // points at a non-default port, and a pre-#710 `:port` id.
    tokio::fs::write(
        dir.join("ssh_connections.json"),
        serde_json::to_string_pretty(&serde_json::json!([
            {
                "id": "ssh-root@cluster",
                "name": "cluster node",
                "host": "cluster",
                "port": 2201,
                "username": "root",
                "authType": { "type": "PrivateKey", "keyPath": "~/.ssh/id_ed25519" },
                "lastConnected": 1
            },
            {
                "id": "ssh-alice@legacy.example:2222",
                "name": "legacy",
                "host": "legacy.example",
                "port": 2222,
                "username": "alice",
                "authType": { "type": "PrivateKey", "keyPath": "~/.ssh/id_ed25519" },
                "lastConnected": 1
            }
        ]))
        .unwrap(),
    )
    .await
    .unwrap();

    let manager = SSHConnectionManager::new(dir.clone());
    manager.load_saved_connections().await.unwrap();
    // A new default-port profile on the same host must not overwrite the
    // legacy profile; the dialog gives it a port-scoped id because the
    // port-less id is already taken by another port.
    manager
        .save_connection(&key_profile("ssh-root@cluster#22", "cluster", 22, "root"))
        .await
        .unwrap();

    let reloaded = SSHConnectionManager::new(dir.clone());
    reloaded.load_saved_connections().await.unwrap();
    let mut saved: Vec<(String, u16)> = reloaded
        .get_saved_connections()
        .await
        .into_iter()
        .map(|c| (c.id, c.port))
        .collect();
    saved.sort();
    assert_eq!(
        saved,
        vec![
            ("ssh-alice@legacy.example:2222".to_string(), 2222),
            ("ssh-root@cluster".to_string(), 2201),
            ("ssh-root@cluster#22".to_string(), 22),
        ]
    );
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn saved_none_auth_profile_round_trips_without_credentials() {
    let dir = test_data_dir("none-auth-profile");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());

    manager
        .save_connection(&SSHConnectionConfig {
            id: "ssh-coder@127.0.0.1".to_string(),
            name: "coder agent".to_string(),
            host: "127.0.0.1".to_string(),
            port: 2222,
            username: "coder".to_string(),
            auth: SSHAuthMethod::None,
            default_workspace: Some("/root".to_string()),
            proxy_jump: None,
            proxy_command: None,
            container: None,
            wsl: None,
            options: Default::default(),
        })
        .await
        .unwrap();

    let persisted = tokio::fs::read_to_string(dir.join(EXTENDED_SAVED_CONNECTIONS_FILE))
        .await
        .unwrap();
    assert!(
        persisted.contains("\"None\""),
        "saved profile must record the none method: {persisted}"
    );
    assert!(
        !manager.has_stored_password("ssh-coder@127.0.0.1").await,
        "none authentication must not create a password vault entry"
    );

    let restored = manager
        .load_connection_config_from_saved("ssh-coder@127.0.0.1")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(restored.auth, SSHAuthMethod::None));

    // Empty bodies are the shape the web UI sends for a new profile.
    let from_wire: SSHConnectionConfig = serde_json::from_value(serde_json::json!({
        "id": "ssh-coder@127.0.0.1",
        "name": "coder agent",
        "host": "127.0.0.1",
        "port": 2222,
        "username": "coder",
        "auth": { "type": "None" },
        "defaultWorkspace": null,
    }))
    .unwrap();
    assert!(matches!(from_wire.auth, SSHAuthMethod::None));
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn prunes_remote_workspaces_without_saved_connection() {
    let dir = test_data_dir("missing-saved");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());

    let workspaces = vec![RemoteWorkspace {
        connection_id: "ssh-root@example.com:22".to_string(),
        remote_path: "/root/project".to_string(),
        connection_name: "root@example.com".to_string(),
        ssh_host: "example.com".to_string(),
    }];
    tokio::fs::write(
        dir.join("remote_workspace.json"),
        serde_json::to_string_pretty(&workspaces).unwrap(),
    )
    .await
    .unwrap();

    manager.load_remote_workspace().await.unwrap();
    let removed = manager
        .prune_remote_workspaces_without_saved_connections()
        .await
        .unwrap();

    assert_eq!(removed.len(), 1);
    assert!(manager.get_remote_workspaces().await.is_empty());
    let _ = tokio::fs::remove_dir_all(&dir).await;
}
