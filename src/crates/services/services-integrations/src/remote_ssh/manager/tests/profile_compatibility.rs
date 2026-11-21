use super::super::*;
use super::support::test_data_dir;

struct NoneOnlyTestServer {
    none_attempts: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl russh::server::Handler for NoneOnlyTestServer {
    type Error = russh::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<russh::server::Auth, Self::Error> {
        self.none_attempts.fetch_add(1, Ordering::SeqCst);
        Ok(russh::server::Auth::Accept)
    }
}

fn nc_on_path() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|path| path.join("nc").is_file()))
}

/// The Coder shape: the transport is a local process and the endpoint
/// advertises only `none`, so any other method would fail the connection.
#[cfg(unix)]
#[tokio::test]
async fn none_auth_over_proxy_command_connects_after_restoring_the_saved_profile() {
    if !nc_on_path() {
        return;
    }
    let dir = test_data_dir("none-auth-proxy-command");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let none_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_attempts = none_attempts.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let config = Arc::new(russh::server::Config {
            keys: vec![KeyPair::generate_ed25519().unwrap()],
            methods: russh::MethodSet::NONE,
            ..Default::default()
        });
        let handler = NoneOnlyTestServer {
            none_attempts: server_attempts,
        };
        let session = russh::server::run_stream(config, socket, handler)
            .await
            .unwrap();
        let _ = session.await;
    });

    // The alias does not resolve, so only the proxy can reach the server.
    let proxy_command = format!("nc 127.0.0.1 {port}");
    let mut profile = none_auth_config("coder", "coder-workspace.invalid");
    profile.proxy_command = Some(proxy_command.clone());
    let manager = SSHConnectionManager::new(dir.clone());
    manager.save_connection(&profile).await.unwrap();

    let reloaded = SSHConnectionManager::new(dir.clone());
    reloaded.load_saved_connections().await.unwrap();
    let restored = reloaded
        .load_connection_config_from_saved("coder")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(restored.auth, SSHAuthMethod::None));
    assert_eq!(restored.proxy_command, Some(proxy_command));

    let result = reloaded.connect(restored).await.unwrap();
    assert!(result.success, "unexpected result: {:?}", result.error);
    assert!(none_attempts.load(Ordering::SeqCst) >= 1);

    let _ = reloaded.disconnect("coder").await;
    server.abort();
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

/// `authType` exactly as released builds parse it: no `None` variant.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type")]
enum ReleasedSavedAuthType {
    Password,
    PrivateKey {
        #[serde(rename = "keyPath")]
        key_path: String,
    },
    Agent,
    KeyboardInteractive,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleasedSavedConnection {
    id: String,
    name: String,
    host: String,
    port: u16,
    username: String,
    auth_type: ReleasedSavedAuthType,
}

fn released_profile(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": id,
        "host": format!("{id}.test"),
        "port": 22,
        "username": "dev",
        "authType": { "type": "PrivateKey", "keyPath": "~/.ssh/id_rsa" },
    })
}

fn none_auth_config(id: &str, host: &str) -> SSHConnectionConfig {
    SSHConnectionConfig {
        id: id.to_string(),
        name: "coder agent".to_string(),
        host: host.to_string(),
        port: 2222,
        username: "coder".to_string(),
        auth: SSHAuthMethod::None,
        default_workspace: Some("/root".to_string()),
        proxy_jump: None,
        proxy_command: None,
        container: None,
        wsl: None,
        options: Default::default(),
    }
}

#[tokio::test]
async fn none_auth_profile_survives_a_released_reader_save_and_upgrade() {
    let dir = test_data_dir("none-auth-downgrade");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let primary_path = dir.join("ssh_connections.json");
    tokio::fs::write(
        &primary_path,
        serde_json::to_vec(&vec![released_profile("old-a")]).unwrap(),
    )
    .await
    .unwrap();

    let upgraded = SSHConnectionManager::new(dir.clone());
    upgraded.load_saved_connections().await.unwrap();
    upgraded
        .save_connection(&none_auth_config("coder", "coder.test"))
        .await
        .unwrap();

    // Downgrade: a released build parses the whole list, then rewrites it
    // from memory after the user adds a profile.
    let content = tokio::fs::read_to_string(&primary_path).await.unwrap();
    let mut released: Vec<ReleasedSavedConnection> =
        serde_json::from_str(&content).expect("released builds must keep parsing the primary file");
    assert_eq!(released.len(), 1);
    released.push(serde_json::from_value(released_profile("old-b")).unwrap());
    tokio::fs::write(&primary_path, serde_json::to_vec(&released).unwrap())
        .await
        .unwrap();

    // Upgrade again: nothing either side wrote is lost.
    let reupgraded = SSHConnectionManager::new(dir.clone());
    reupgraded.load_saved_connections().await.unwrap();
    let mut ids: Vec<String> = reupgraded
        .get_saved_connections()
        .await
        .into_iter()
        .map(|connection| connection.id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["coder", "old-a", "old-b"]);
    let restored = reupgraded
        .load_connection_config_from_saved("coder")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(restored.auth, SSHAuthMethod::None));
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn none_profiles_keep_port_scoped_ids_and_legacy_alias_dedup_across_stores() {
    let dir = test_data_dir("none-auth-port-ids");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());
    let mut key_profile = none_auth_config("ssh-coder@ws", "ws");
    key_profile.port = 22;
    key_profile.auth = SSHAuthMethod::Agent {
        key_fingerprint: None,
        fallback_key_path: None,
    };
    manager.save_connection(&key_profile).await.unwrap();
    let mut scoped = none_auth_config("ssh-coder@ws#2222", "ws");
    scoped.port = 2222;
    manager.save_connection(&scoped).await.unwrap();

    // Two ports on one host stay distinct even though they live in
    // different stores.
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
            ("ssh-coder@ws".to_string(), 22),
            ("ssh-coder@ws#2222".to_string(), 2222),
        ]
    );
    let restored = reloaded
        .load_connection_config_from_saved("ssh-coder@ws#2222")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(restored.auth, SSHAuthMethod::None));

    // A legacy `:port` alias of the key profile switched to `None`
    // replaces it and moves it to the extended store.
    let mut alias = none_auth_config("ssh-coder@ws:22", "ws");
    alias.port = 22;
    reloaded.save_connection(&alias).await.unwrap();
    let primary = tokio::fs::read_to_string(dir.join("ssh_connections.json"))
        .await
        .unwrap();
    assert!(
        serde_json::from_str::<Vec<ReleasedSavedConnection>>(&primary)
            .unwrap()
            .is_empty()
    );
    let reloaded = SSHConnectionManager::new(dir.clone());
    reloaded.load_saved_connections().await.unwrap();
    let mut ids: Vec<String> = reloaded
        .get_saved_connections()
        .await
        .into_iter()
        .map(|c| c.id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["ssh-coder@ws#2222", "ssh-coder@ws:22"]);
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn primary_alias_from_an_older_build_shadows_the_same_extended_endpoint() {
    let dir = test_data_dir("none-auth-alias-shadow");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());
    let mut none_profile = none_auth_config("ssh-coder@ws", "ws");
    none_profile.port = 22;
    manager.save_connection(&none_profile).await.unwrap();
    let mut alias = released_profile("ssh-coder@ws:22");
    alias["host"] = serde_json::json!("ws");
    alias["username"] = serde_json::json!("coder");
    let mut other_port = released_profile("ssh-coder@ws:2201");
    other_port["host"] = serde_json::json!("ws");
    other_port["username"] = serde_json::json!("coder");
    other_port["port"] = serde_json::json!(2201);
    tokio::fs::write(
        dir.join("ssh_connections.json"),
        serde_json::to_vec(&vec![alias, other_port]).unwrap(),
    )
    .await
    .unwrap();

    let reloaded = SSHConnectionManager::new(dir.clone());
    reloaded.load_saved_connections().await.unwrap();
    let mut ids: Vec<String> = reloaded
        .get_saved_connections()
        .await
        .into_iter()
        .map(|c| c.id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["ssh-coder@ws:22", "ssh-coder@ws:2201"]);
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn none_profile_in_the_primary_file_moves_to_the_extended_store() {
    let dir = test_data_dir("none-auth-relocate");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let primary_path = dir.join("ssh_connections.json");
    let mut none_profile = released_profile("coder");
    none_profile["authType"] = serde_json::json!({ "type": "None" });
    tokio::fs::write(
        &primary_path,
        serde_json::to_vec(&vec![released_profile("old-a"), none_profile]).unwrap(),
    )
    .await
    .unwrap();

    let manager = SSHConnectionManager::new(dir.clone());
    manager.load_saved_connections().await.unwrap();
    assert_eq!(manager.get_saved_connections().await.len(), 2);
    manager.delete_saved_connection("old-a").await.unwrap();

    let content = tokio::fs::read_to_string(&primary_path).await.unwrap();
    serde_json::from_str::<Vec<ReleasedSavedConnection>>(&content)
        .expect("a saved primary file must stay readable by released builds");
    let extended = tokio::fs::read_to_string(dir.join(EXTENDED_SAVED_CONNECTIONS_FILE))
        .await
        .unwrap();
    assert!(extended.contains("\"coder\""));
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn changing_auth_moves_a_profile_between_stores_and_delete_clears_it() {
    let dir = test_data_dir("none-auth-switch");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());
    manager
        .save_connection(&none_auth_config("coder", "coder.test"))
        .await
        .unwrap();
    let mut with_key = none_auth_config("coder", "coder.test");
    with_key.auth = SSHAuthMethod::Agent {
        key_fingerprint: None,
        fallback_key_path: None,
    };
    manager.save_connection(&with_key).await.unwrap();

    let reloaded = SSHConnectionManager::new(dir.clone());
    reloaded.load_saved_connections().await.unwrap();
    let saved = reloaded.get_saved_connections().await;
    assert_eq!(saved.len(), 1);
    assert!(!matches!(
        saved[0].auth_type,
        crate::remote_ssh::types::SavedAuthType::None
    ));

    reloaded
        .save_connection(&none_auth_config("coder", "coder.test"))
        .await
        .unwrap();
    reloaded.delete_saved_connection("coder").await.unwrap();
    let reloaded = SSHConnectionManager::new(dir.clone());
    reloaded.load_saved_connections().await.unwrap();
    assert!(reloaded.get_saved_connections().await.is_empty());
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn primary_profile_recreated_by_an_older_build_shadows_the_extended_one() {
    let dir = test_data_dir("none-auth-shadow");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());
    manager
        .save_connection(&none_auth_config("coder", "coder.test"))
        .await
        .unwrap();
    tokio::fs::write(
        dir.join("ssh_connections.json"),
        serde_json::to_vec(&vec![released_profile("coder")]).unwrap(),
    )
    .await
    .unwrap();

    let reloaded = SSHConnectionManager::new(dir.clone());
    reloaded.load_saved_connections().await.unwrap();
    let saved = reloaded.get_saved_connections().await;
    assert_eq!(saved.len(), 1);
    assert!(matches!(
        saved[0].auth_type,
        crate::remote_ssh::types::SavedAuthType::PrivateKey { .. }
    ));
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn unreadable_extended_store_is_kept_aside_and_never_blocks_primary_profiles() {
    let dir = test_data_dir("none-auth-unreadable");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(
        dir.join("ssh_connections.json"),
        serde_json::to_vec(&vec![released_profile("old-a")]).unwrap(),
    )
    .await
    .unwrap();
    let extended_path = dir.join(EXTENDED_SAVED_CONNECTIONS_FILE);
    tokio::fs::write(&extended_path, "not json").await.unwrap();

    let manager = SSHConnectionManager::new(dir.clone());
    manager.load_saved_connections().await.unwrap();
    assert_eq!(manager.get_saved_connections().await.len(), 1);
    manager
        .save_connection(&none_auth_config("coder", "coder.test"))
        .await
        .unwrap();

    let backup = tokio::fs::read_to_string(extended_path.with_extension("json.unreadable"))
        .await
        .unwrap();
    assert_eq!(backup, "not json");
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[tokio::test]
async fn unreadable_primary_file_keeps_extended_profiles_for_the_next_save() {
    let dir = test_data_dir("none-auth-primary-corrupt");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());
    manager
        .save_connection(&none_auth_config("coder", "coder.test"))
        .await
        .unwrap();
    tokio::fs::write(dir.join("ssh_connections.json"), "not json")
        .await
        .unwrap();

    let reloaded = SSHConnectionManager::new(dir.clone());
    assert!(reloaded.load_saved_connections().await.is_err());
    assert_eq!(reloaded.get_saved_connections().await.len(), 1);
    let _ = tokio::fs::remove_dir_all(&dir).await;
}
