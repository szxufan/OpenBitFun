use super::super::*;

#[tokio::test]
async fn workspace_command_owner_survives_caller_drop_until_cleanup_finishes() {
    let parent = tokio_util::sync::CancellationToken::new();
    let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let resource_dropped = Arc::new(AtomicBool::new(false));
    struct CommandResource(Arc<AtomicBool>);
    impl Drop for CommandResource {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (cancelled_tx, cancelled_rx) = tokio::sync::oneshot::channel();
    let (finish_cleanup_tx, finish_cleanup_rx) = tokio::sync::oneshot::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let observed_executions = executions.clone();
    let observed_resource = resource_dropped.clone();
    let options = SSHCommandOptions {
        timeout_ms: None,
        cancellation_token: Some(parent.clone()),
    };
    let caller = tokio::spawn(supervise_ssh_command(options, move |options| async move {
        let resource = CommandResource(observed_resource);
        observed_executions.fetch_add(1, Ordering::SeqCst);
        started_tx.send(()).unwrap();
        options.cancellation_token.unwrap().cancelled().await;
        cancelled_tx.send(()).unwrap();
        // Model asynchronous INT/drain/close work that must remain owned
        // after the caller's timeout drops its future.
        finish_cleanup_rx.await.unwrap();
        drop(resource);
        finished_tx.send(()).unwrap();
        Ok(SshCommandStop::Cancelled.output())
    }));
    tokio::time::timeout(Duration::from_secs(2), started_rx)
        .await
        .unwrap()
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), cancelled_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(!resource_dropped.load(Ordering::SeqCst));
    assert!(
        !parent.is_cancelled(),
        "a cancelled command must not cancel sibling commands or its turn"
    );
    finish_cleanup_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), finished_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(resource_dropped.load(Ordering::SeqCst));
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn workspace_command_owner_observes_parent_cancellation_and_preserves_results() {
    let parent = tokio_util::sync::CancellationToken::new();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let options = SSHCommandOptions {
        timeout_ms: Some(1234),
        cancellation_token: Some(parent.clone()),
    };
    let caller = tokio::spawn(supervise_ssh_command(options, move |options| async move {
        assert_eq!(options.timeout_ms, Some(1234));
        started_tx.send(()).unwrap();
        options.cancellation_token.unwrap().cancelled().await;
        let mut result = SshCommandStop::Cancelled.result();
        result.stdout = "partial output".into();
        result.stderr = "interrupted by caller".into();
        Ok(SshCommandOutput {
            result,
            exit_status_received: false,
        })
    }));
    tokio::time::timeout(Duration::from_secs(2), started_rx)
        .await
        .unwrap()
        .unwrap();
    parent.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), caller)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(result.interrupted);
    assert!(!result.timed_out);
    assert_eq!(result.exit_code, 130);
    assert_eq!(result.stdout, "partial output");
    assert_eq!(result.stderr, "interrupted by caller");
}

#[tokio::test]
async fn workspace_command_owner_keeps_normal_and_unlimited_call_behavior() {
    let result = supervise_ssh_command(SSHCommandOptions::default(), |options| async move {
        assert!(options.timeout_ms.is_none());
        assert!(!options.cancellation_token.unwrap().is_cancelled());
        Ok(SshCommandOutput {
            result: SSHCommandResult {
                stdout: "done".into(),
                stderr: "diagnostic".into(),
                exit_code: 7,
                interrupted: false,
                timed_out: false,
            },
            exit_status_received: true,
        })
    })
    .await
    .unwrap();
    assert_eq!(result.exit_code, 7);
    assert_eq!(result.stdout, "done");
    assert_eq!(result.stderr, "diagnostic");
    assert!(!result.interrupted && !result.timed_out);
    let error = supervise_ssh_command(SSHCommandOptions::default(), |_| async {
        Err(anyhow!("transport disconnected"))
    })
    .await
    .unwrap_err();
    assert!(error.to_string().contains("transport disconnected"));
}

#[tokio::test]
async fn workspace_command_deadline_cancels_owner_immediately_and_keeps_timeout_reason() {
    let started_at = Instant::now();
    let result = supervise_ssh_command(
        SSHCommandOptions {
            timeout_ms: Some(30),
            cancellation_token: None,
        },
        |options| async move {
            tokio::time::timeout(
                Duration::from_millis(500),
                options.cancellation_token.unwrap().cancelled(),
            )
            .await
            .expect("deadline must cancel before the owner-drain grace expires");
            let mut result = SshCommandStop::Cancelled.result();
            result.stdout = "partial output".into();
            Ok(SshCommandOutput {
                result,
                exit_status_received: false,
            })
        },
    )
    .await
    .unwrap();
    assert!(started_at.elapsed() < SSH_COMMAND_OWNER_DRAIN_GRACE);
    assert_eq!(result.stdout, "partial output");
    assert_eq!(result.exit_code, 124);
    assert!(result.timed_out);
    assert!(!result.interrupted);
}

#[tokio::test]
async fn workspace_command_supervisor_preserves_real_exit_codes_when_stopped() {
    for stop in [SshCommandStop::Cancelled, SshCommandStop::TimedOut] {
        for exit_code in [-1, 124, 130] {
            let cancellation = tokio_util::sync::CancellationToken::new();
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let caller = tokio::spawn(supervise_ssh_command(
                SSHCommandOptions {
                    timeout_ms: (stop == SshCommandStop::TimedOut).then_some(30),
                    cancellation_token: Some(cancellation.clone()),
                },
                move |options| async move {
                    let _ = started_tx.send(());
                    options.cancellation_token.unwrap().cancelled().await;
                    Ok(SshCommandOutput {
                        result: SSHCommandResult {
                            stdout: "partial output".into(),
                            stderr: String::new(),
                            exit_code,
                            interrupted: true,
                            timed_out: false,
                        },
                        exit_status_received: true,
                    })
                },
            ));
            started_rx.await.unwrap();
            if stop == SshCommandStop::Cancelled {
                cancellation.cancel();
            }
            let result = tokio::time::timeout(Duration::from_secs(2), caller)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(result.exit_code, exit_code);
            assert_eq!(result.stdout, "partial output");
            assert_eq!(result.interrupted, stop == SshCommandStop::Cancelled);
            assert_eq!(result.timed_out, stop == SshCommandStop::TimedOut);
        }
    }
}

#[tokio::test]
async fn workspace_command_phase_honors_stop_before_polling_or_while_pending() {
    let cancelled = tokio_util::sync::CancellationToken::new();
    cancelled.cancel();
    let mut polled = false;
    let outcome = await_ssh_command_phase(
        std::future::poll_fn(|_| {
            polled = true;
            std::task::Poll::Ready(())
        }),
        Some(&cancelled),
        None,
    )
    .await;
    assert_eq!(outcome, Err(SshCommandStop::Cancelled));
    assert!(
        !polled,
        "a pre-cancelled command must not open a channel or submit exec"
    );
    let outcome = await_ssh_command_phase(
        std::future::pending::<()>(),
        None,
        Some(Instant::now() + Duration::from_millis(1)),
    )
    .await;
    assert_eq!(outcome, Err(SshCommandStop::TimedOut));
    let outcome = await_ssh_command_phase(std::future::ready(17), None, None).await;
    assert_eq!(outcome, Ok(17));

    let token = tokio_util::sync::CancellationToken::new();
    let worker_token = token.clone();
    let (pending_tx, pending_rx) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        await_ssh_command_phase(
            async {
                pending_tx.send(()).unwrap();
                std::future::pending::<()>().await
            },
            Some(&worker_token),
            None,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), pending_rx)
        .await
        .unwrap()
        .unwrap();
    token.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap(),
        Err(SshCommandStop::Cancelled)
    );
}

struct DelayedWorkspaceOpenServer {
    first_open: Option<(
        tokio::sync::oneshot::Sender<russh::ChannelId>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
    closed: tokio::sync::mpsc::UnboundedSender<russh::ChannelId>,
    executions: Arc<std::sync::atomic::AtomicUsize>,
    keep_exec_open: bool,
}

#[async_trait]
impl russh::server::Handler for DelayedWorkspaceOpenServer {
    type Error = russh::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<russh::server::Auth, Self::Error> {
        Ok(russh::server::Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<russh::server::Msg>,
        _session: &mut russh::server::Session,
    ) -> Result<bool, Self::Error> {
        if let Some((requested, release)) = self.first_open.take() {
            requested.send(channel.id()).unwrap();
            release.await.unwrap();
        }
        Ok(true)
    }

    async fn channel_close(
        &mut self,
        channel: russh::ChannelId,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        let _ = self.closed.send(channel);
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        _data: &[u8],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        session.channel_success(channel);
        if self.keep_exec_open {
            return Ok(());
        }
        session.data(
            channel,
            russh::CryptoVec::from_slice(b"sibling command output"),
        );
        session.exit_status_request(channel, 0);
        session.eof(channel);
        session.close(channel);
        Ok(())
    }
}

async fn assert_workspace_late_open_is_closed(stop: SshCommandStop, docker_setup: bool) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (requested_tx, requested_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (closed_tx, mut closed_rx) = tokio::sync::mpsc::unbounded_channel();
    let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server = DelayedWorkspaceOpenServer {
        first_open: Some((requested_tx, release_rx)),
        closed: closed_tx,
        executions: executions.clone(),
        keep_exec_open: false,
    };
    let server_config = Arc::new(russh::server::Config {
        keys: vec![KeyPair::generate_ed25519().unwrap()],
        ..Default::default()
    });
    let server_task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        russh::server::run_stream(server_config, socket, server)
            .await
            .unwrap()
            .await
    });
    let mut handle = russh::client::connect(
        Arc::new(russh::client::Config::default()),
        address,
        SSHHandler::with_verify_callback(|_, _, _| true),
    )
    .await
    .unwrap();
    assert!(handle.authenticate_none("workspace-test").await.unwrap());
    let handle = Arc::new(handle);
    let cancellation = tokio_util::sync::CancellationToken::new();
    let options = SSHCommandOptions {
        timeout_ms: (stop == SshCommandStop::TimedOut).then_some(1_000),
        cancellation_token: Some(cancellation.clone()),
    };
    let owner_handle = handle.clone();
    let fixture = tempfile::tempdir().unwrap();
    let caller = if docker_setup {
        let manager = SSHConnectionManager::new(fixture.path().into());
        let config = SSHConnectionConfig {
            id: "workspace-late-open".into(),
            name: "loopback SSH fixture".into(),
            host: "127.0.0.1".into(),
            port: address.port(),
            username: "workspace-test".into(),
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
                local: false,
                docker_path: "docker".into(),
                shell: "/bin/sh".into(),
                user: None,
                interactive: true,
            }),
            wsl: None,
            options: Default::default(),
        };
        manager.connections.write().await.insert(
            config.id.clone(),
            ActiveConnection {
                handle: Some(owner_handle),
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
        tokio::spawn(async move {
            manager
                .execute_command_with_options("workspace-late-open", "must never execute", options)
                .await
        })
    } else {
        // Exercise the formerly unsupervised internal entry directly.
        tokio::spawn(async move {
            SSHConnectionManager::execute_command_internal(
                &owner_handle,
                "must never execute",
                options,
            )
            .await
        })
    };
    let late_channel = tokio::time::timeout(Duration::from_secs(5), requested_rx)
        .await
        .unwrap()
        .unwrap();
    let confirmation_delayed_at = Instant::now();
    if stop == SshCommandStop::Cancelled {
        cancellation.cancel();
    }
    let result = tokio::time::timeout(Duration::from_secs(8), caller)
        .await
        .expect("caller must finish without waiting for channel-open confirmation")
        .unwrap()
        .unwrap();
    assert_eq!(result.exit_code, stop.result().exit_code);
    assert_eq!(result.interrupted, stop == SshCommandStop::Cancelled);
    assert_eq!(result.timed_out, stop == SshCommandStop::TimedOut);
    assert!(confirmation_delayed_at.elapsed() >= SSH_COMMAND_INTERRUPT_DRAIN_GRACE);
    assert_eq!(executions.load(Ordering::SeqCst), 0);

    // Confirmation arrives only after the caller has returned, beyond the
    // old 500ms cleanup window. The detached owner must still send CLOSE.
    release_tx.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), closed_rx.recv())
            .await
            .expect("late confirmation must be closed rather than orphaned"),
        Some(late_channel)
    );
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert!(
        !handle.is_closed(),
        "cleanup must preserve the shared connection"
    );

    let sibling_handle = handle.clone();
    let sibling = SSHConnectionManager::execute_command_internal(
        &sibling_handle,
        "sibling command",
        SSHCommandOptions {
            timeout_ms: Some(2_000),
            cancellation_token: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(sibling.exit_code, 0);
    assert_eq!(sibling.stdout, "sibling command output");
    assert!(!sibling.interrupted && !sibling.timed_out);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    handle
        .disconnect(russh::Disconnect::ByApplication, "test finished", "")
        .await
        .unwrap();
    let server_result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("test transport must close")
        .unwrap();
    assert_fixture_server_disconnected(server_result);
}

#[tokio::test]
async fn workspace_command_cancel_closes_late_confirmation_without_executing() {
    assert_workspace_late_open_is_closed(SshCommandStop::Cancelled, false).await;
}

#[tokio::test]
async fn workspace_command_timeout_closes_late_confirmation_without_executing() {
    assert_workspace_late_open_is_closed(SshCommandStop::TimedOut, false).await;
}

#[tokio::test]
async fn workspace_docker_setup_cancel_closes_late_confirmation_without_executing() {
    assert_workspace_late_open_is_closed(SshCommandStop::Cancelled, true).await;
}

#[tokio::test]
async fn workspace_docker_setup_timeout_closes_late_confirmation_without_executing() {
    assert_workspace_late_open_is_closed(SshCommandStop::TimedOut, true).await;
}

#[tokio::test]
async fn workspace_exec_channel_closes_when_completed_handoff_is_not_consumed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (closed_tx, mut closed_rx) = tokio::sync::mpsc::unbounded_channel();
    let server = DelayedWorkspaceOpenServer {
        first_open: None,
        closed: closed_tx,
        executions: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        keep_exec_open: true,
    };
    let server_config = Arc::new(russh::server::Config {
        keys: vec![KeyPair::generate_ed25519().unwrap()],
        ..Default::default()
    });
    let server_task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        russh::server::run_stream(server_config, socket, server)
            .await
            .unwrap()
            .await
    });
    let mut handle = russh::client::connect(
        Arc::new(russh::client::Config::default()),
        address,
        SSHHandler::with_verify_callback(|_, _, _| true),
    )
    .await
    .unwrap();
    assert!(handle.authenticate_none("workspace-test").await.unwrap());
    let handle = Arc::new(handle);
    let owner_handle = handle.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let mut caller = Box::pin(supervise_ssh_owner(
        SSHCommandOptions::default(),
        move |options| async move {
            let channel = SSHConnectionManager::open_ssh_exec_channel_owned(
                &owner_handle,
                "hold open",
                options,
                None,
            )
            .await
            .unwrap();
            let handoff = SshExecChannelHandoff::new(channel);
            ready_tx
                .send(handoff.channel.as_ref().unwrap().id())
                .unwrap();
            handoff
        },
    ));
    // Start the owner, but never poll the supervisor again to consume its
    // result. On this current-thread runtime, the owner completes in the
    // same poll that sends ready_tx, before this test resumes.
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(caller.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    let channel_id = tokio::time::timeout(Duration::from_secs(5), ready_rx)
        .await
        .unwrap()
        .unwrap();
    drop(caller);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), closed_rx.recv())
            .await
            .unwrap(),
        Some(channel_id)
    );
    assert!(
        !handle.is_closed(),
        "abandoning one handoff must not disconnect siblings"
    );
    handle
        .disconnect(russh::Disconnect::ByApplication, "fixture complete", "en")
        .await
        .unwrap();
    let server_result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .unwrap()
        .unwrap();
    assert_fixture_server_disconnected(server_result);
}

fn assert_fixture_server_disconnected(result: Result<(), russh::Error>) {
    // Only used after the assertions and an explicit client disconnect.
    // russh may observe the closing transport before the disconnect frame;
    // Darwin reports that race as ConnectionReset instead of UnexpectedEof.
    match result {
        Ok(()) => {}
        Err(russh::Error::IO(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
            ) => {}
        Err(error) => panic!("SSH fixture failed during disconnect: {error:?}"),
    }
}
