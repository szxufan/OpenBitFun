use super::super::*;
use super::support::test_data_dir;

struct UnpublishedSessionTestServer {
    opens: usize,
    delayed_open: Option<usize>,
    requested: Option<tokio::sync::oneshot::Sender<()>>,
    release: Option<tokio::sync::oneshot::Receiver<()>>,
}

#[async_trait]
impl russh::server::Handler for UnpublishedSessionTestServer {
    type Error = russh::Error;

    async fn auth_password(
        &mut self,
        _user: &str,
        _password: &str,
    ) -> Result<russh::server::Auth, Self::Error> {
        Ok(russh::server::Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _channel: russh::Channel<russh::server::Msg>,
        _session: &mut russh::server::Session,
    ) -> Result<bool, Self::Error> {
        self.opens += 1;
        if self.delayed_open == Some(self.opens) {
            let _ = self.requested.take().unwrap().send(());
            let _ = self.release.take().unwrap().await;
        }
        Ok(true)
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: russh::Channel<russh::server::Msg>,
        host: &str,
        port: u32,
        _originator_host: &str,
        _originator_port: u32,
        _session: &mut russh::server::Session,
    ) -> Result<bool, Self::Error> {
        let mut socket = tokio::net::TcpStream::connect((host, port as u16)).await?;
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut socket).await;
        });
        Ok(true)
    }

    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        _command: &[u8],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel);
        session.data(
            channel,
            russh::CryptoVec::from_slice(b"Linux\nfixture\n/workspace\n"),
        );
        session.exit_status_request(channel, 0);
        session.eof(channel);
        session.close(channel);
        Ok(())
    }
}

struct UnpublishedSessionFixture {
    config: SSHConnectionConfig,
    requested: tokio::sync::oneshot::Receiver<()>,
    release: tokio::sync::oneshot::Sender<()>,
    closed: tokio::sync::oneshot::Receiver<()>,
    server: tokio::task::JoinHandle<()>,
    proxy: tokio::task::JoinHandle<()>,
}

async fn unpublished_session_fixture(
    manager: &SSHConnectionManager,
    delayed_open: Option<usize>,
) -> UnpublishedSessionFixture {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let key = KeyPair::generate_ed25519().unwrap();
    manager
        .add_known_host(
            "127.0.0.1".into(),
            address.port(),
            &key.clone_public_key().unwrap(),
        )
        .await
        .unwrap();
    let (requested_tx, requested) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    let (closed_tx, closed) = tokio::sync::oneshot::channel();
    let (server_stream, proxy_stream) = tokio::io::duplex(65_536);
    let server = tokio::spawn(async move {
        let handler = UnpublishedSessionTestServer {
            opens: 0,
            delayed_open,
            requested: Some(requested_tx),
            release: Some(release_rx),
        };
        let config = Arc::new(russh::server::Config {
            keys: vec![key],
            ..Default::default()
        });
        let session = russh::server::run_stream(config, server_stream, handler)
            .await
            .unwrap();
        let _ = session.await;
    });
    // Observe transport EOF independently of the server's deliberately
    // blocked channel-open handler. Merely releasing that handler would
    // let an orphaned owner finish and hide the lifetime regression.
    let proxy = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (mut socket_read, mut socket_write) = socket.into_split();
        let (mut server_read, mut server_write) = tokio::io::split(proxy_stream);
        let inbound = async move {
            let _ = tokio::io::copy(&mut socket_read, &mut server_write).await;
            let _ = closed_tx.send(());
            let _ = server_write.shutdown().await;
        };
        let outbound = async move {
            let _ = tokio::io::copy(&mut server_read, &mut socket_write).await;
            let _ = socket_write.shutdown().await;
        };
        tokio::join!(inbound, outbound);
    });
    UnpublishedSessionFixture {
        config: SSHConnectionConfig {
            id: format!("unpublished-{}", address.port()),
            name: "Unpublished SSH fixture".into(),
            host: "127.0.0.1".into(),
            port: address.port(),
            username: "workspace-test".into(),
            auth: SSHAuthMethod::Password {
                password: "test-fixture".into(),
            },
            default_workspace: None,
            proxy_jump: None,
            proxy_command: None,
            container: None,
            wsl: None,
            options: Default::default(),
        },
        requested,
        release,
        closed,
        server,
        proxy,
    }
}

async fn finish_unpublished_fixture(fixture: UnpublishedSessionFixture) {
    let _ = fixture.release.send(());
    tokio::time::timeout(Duration::from_secs(5), fixture.server)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), fixture.proxy)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn workspace_unpublished_probe_error_closes_transport_before_late_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let manager = SSHConnectionManager::new(temp.path().into());
    // Metadata succeeds; the container probe's channel-open never replies.
    let mut fixture = unpublished_session_fixture(&manager, Some(2)).await;
    let mut config = fixture.config.clone();
    config.container = Some(ContainerWorkspaceConfig {
        name: "fixture".into(),
        access: ContainerAccess::DockerExec,
        local: false,
        docker_path: "docker".into(),
        shell: "/bin/sh".into(),
        user: None,
        interactive: true,
    });
    let caller = tokio::spawn(async move { manager.establish_session(&config, 1).await });
    tokio::time::timeout(Duration::from_secs(5), &mut fixture.requested)
        .await
        .unwrap()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(6), caller)
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.is_err(),
        "the failed container probe must not publish a connection"
    );
    tokio::time::timeout(Duration::from_secs(3), &mut fixture.closed)
        .await
        .expect("failed temporary transport must close before the late open is released")
        .unwrap();
    finish_unpublished_fixture(fixture).await;
}

#[tokio::test]
async fn workspace_unpublished_caller_drop_closes_target_and_jump_chain() {
    let temp = tempfile::tempdir().unwrap();
    let manager = SSHConnectionManager::new(temp.path().into());
    let mut first = unpublished_session_fixture(&manager, None).await;
    let mut second = unpublished_session_fixture(&manager, None).await;
    let mut target = unpublished_session_fixture(&manager, Some(1)).await;
    let mut config = target.config.clone();
    config.proxy_jump = Some(format!(
        "workspace-test@127.0.0.1:{},workspace-test@127.0.0.1:{}",
        first.config.port, second.config.port,
    ));
    let caller = tokio::spawn(async move { manager.establish_session(&config, 30).await });
    tokio::time::timeout(Duration::from_secs(10), &mut target.requested)
        .await
        .unwrap()
        .unwrap();
    caller.abort();
    assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
    for closed in [&mut target.closed, &mut second.closed, &mut first.closed] {
        tokio::time::timeout(Duration::from_secs(5), closed)
            .await
            .expect("cancelling an unpublished chain must close target and every jump")
            .unwrap();
    }
    finish_unpublished_fixture(target).await;
    finish_unpublished_fixture(second).await;
    finish_unpublished_fixture(first).await;
}

#[test]
fn liveness_probe_is_due_when_stale_suspect_or_clock_moved_back() {
    let liveness = ConnectionLiveness::confirmed_now();
    let confirmed = 1_000_000;
    liveness.confirm(confirmed);
    assert!(!liveness.probe_due(confirmed + LIVENESS_PROBE_AFTER_MS - 1));
    assert!(liveness.probe_due(confirmed + LIVENESS_PROBE_AFTER_MS));
    // A sleep shows up as a wall-clock jump of minutes or hours.
    assert!(liveness.probe_due(confirmed + 6 * 60 * 60 * 1000));
    assert!(liveness.probe_due(confirmed - 1));
    liveness.mark_suspect();
    assert!(liveness.probe_due(confirmed + 1));
    liveness.confirm(confirmed + 2);
    assert!(!liveness.probe_due(confirmed + 3));
}

#[tokio::test]
async fn liveness_probe_separates_refusals_failures_and_silence() {
    assert_eq!(
        classify_probe_result::<()>(Ok(Err(russh::Error::ChannelOpenFailure(
            russh::ChannelOpenFailure::AdministrativelyProhibited,
        )))),
        TransportProbe::Answered
    );
    assert_eq!(
        classify_probe_result::<()>(Ok(Err(russh::Error::Disconnect))),
        TransportProbe::Unresponsive
    );
    let elapsed = tokio::time::timeout(Duration::ZERO, std::future::pending::<()>())
        .await
        .unwrap_err();
    // Silence is not yet proof of death: a busy link gets a patient retry.
    assert_eq!(
        classify_probe_result::<()>(Err(elapsed)),
        TransportProbe::TimedOut
    );
}

async fn age_liveness(manager: &SSHConnectionManager, id: &str) {
    let guard = manager.connections.read().await;
    let liveness = &guard.get(id).unwrap().liveness;
    liveness.confirm(wall_clock_ms() - LIVENESS_PROBE_AFTER_MS - 1);
}

#[tokio::test]
async fn is_connected_actively_confirms_a_stale_transport() {
    let temp = tempfile::tempdir().unwrap();
    let manager = SSHConnectionManager::new(temp.path().into());
    let fixture = unpublished_session_fixture(&manager, None).await;
    let id = fixture.config.id.clone();
    assert!(
        manager
            .connect_with_timeout(fixture.config.clone(), 2)
            .await
            .unwrap()
            .success
    );
    age_liveness(&manager, &id).await;
    assert!(manager.is_connected(&id).await);
    {
        let guard = manager.connections.read().await;
        assert!(!guard.get(&id).unwrap().liveness.probe_due(wall_clock_ms()));
    }
    manager.disconnect(&id).await.unwrap();
    finish_unpublished_fixture(fixture).await;
}

#[tokio::test]
async fn is_connected_reports_a_silent_transport_as_disconnected() {
    let temp = tempfile::tempdir().unwrap();
    let manager = SSHConnectionManager::new(temp.path().into());
    // The publication metadata exec is the first channel; the liveness
    // probe is the second and never gets an answer, like a link after sleep.
    let fixture = unpublished_session_fixture(&manager, Some(2)).await;
    let id = fixture.config.id.clone();
    assert!(
        manager
            .connect_with_timeout(fixture.config.clone(), 2)
            .await
            .unwrap()
            .success
    );
    // A fresh confirmation is trusted without another round trip.
    assert!(manager.is_connected(&id).await);
    manager.mark_connection_suspect(&id).await;
    let started = std::time::Instant::now();
    assert!(!manager.is_connected(&id).await);
    assert!(started.elapsed() < LIVENESS_PROBE_TIMEOUT * 4);
    // The flag stays down, so the next operation reconnects.
    assert!(!manager.is_connected(&id).await);
    manager.disconnect(&id).await.unwrap();
    finish_unpublished_fixture(fixture).await;
}

#[tokio::test]
async fn workspace_unpublished_guard_is_disarmed_after_publication() {
    let temp = tempfile::tempdir().unwrap();
    let manager = SSHConnectionManager::new(temp.path().into());
    let mut fixture = unpublished_session_fixture(&manager, None).await;
    let id = fixture.config.id.clone();
    assert!(
        manager
            .connect_with_timeout(fixture.config.clone(), 2)
            .await
            .unwrap()
            .success
    );
    let result = manager
        .execute_command_with_options(
            &id,
            "published command",
            SSHCommandOptions {
                timeout_ms: Some(2_000),
                cancellation_token: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("/workspace"));
    assert!(matches!(
        fixture.closed.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    manager.disconnect(&id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), &mut fixture.closed)
        .await
        .unwrap()
        .unwrap();
    finish_unpublished_fixture(fixture).await;
}

#[tokio::test]
async fn explicit_connect_reuses_a_matching_live_connection() {
    let manager = SSHConnectionManager::new(test_data_dir("connect-idempotent"));
    let config = SSHConnectionConfig {
        id: "ssh-root@example.com".to_string(),
        name: "example".to_string(),
        host: "example.com".to_string(),
        port: 22,
        username: "root".to_string(),
        auth: SSHAuthMethod::PrivateKey {
            key_path: "/tmp/id_rsa".to_string(),
            passphrase: None,
            certificate_path: None,
        },
        default_workspace: Some("/srv/project".to_string()),
        proxy_jump: None,
        proxy_command: None,
        container: None,
        wsl: None,
        options: Default::default(),
    };
    let alive = Arc::new(AtomicBool::new(true));
    let server_info = ServerInfo {
        os_type: "Linux".to_string(),
        hostname: "example".to_string(),
        home_dir: "/root".to_string(),
    };
    manager.connections.write().await.insert(
        config.id.clone(),
        ActiveConnection {
            handle: None,
            jump_handles: Vec::new(),
            config: config.clone(),
            effective_config: config.clone(),
            server_info: Some(server_info.clone()),
            sftp_session: Arc::new(SftpCache::new()),
            bounded_sftp_session: Arc::new(BoundedSftpCache::new()),
            server_key: None,
            proxy_child: None,
            alive: alive.clone(),
            liveness: Arc::new(ConnectionLiveness::confirmed_now()),
        },
    );

    let result = manager
        .connect_with_timeout(config.clone(), 1)
        .await
        .unwrap();

    assert!(result.success);
    assert_eq!(result.connection_id.as_deref(), Some(config.id.as_str()));
    assert_eq!(
        result
            .server_info
            .as_ref()
            .map(|info| info.hostname.as_str()),
        Some(server_info.hostname.as_str())
    );
    let guard = manager.connections.read().await;
    let active = guard.get(&config.id).unwrap();
    assert!(Arc::ptr_eq(&active.alive, &alive));
}
