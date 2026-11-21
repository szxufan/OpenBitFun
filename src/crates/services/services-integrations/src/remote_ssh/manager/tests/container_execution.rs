use super::super::*;
use super::support::test_data_dir;

#[test]
fn parses_docker_published_ipv4_and_ipv6_endpoints() {
    assert_eq!(
        parse_docker_published_endpoint("0.0.0.0:22022\n[::]:22022\n"),
        Some(("127.0.0.1".to_string(), 22022))
    );
    assert_eq!(
        parse_docker_published_endpoint("192.168.50.2:22023\n"),
        Some(("192.168.50.2".to_string(), 22023))
    );
    assert_eq!(
        parse_docker_published_endpoint("[fd00::10]:22024\n"),
        Some(("fd00::10".to_string(), 22024))
    );
}

#[test]
fn docker_exec_command_quotes_all_user_controlled_values() {
    let container = ContainerWorkspaceConfig {
        name: "dev container".to_string(),
        access: ContainerAccess::DockerExec,
        local: false,
        docker_path: "/opt/docker cli".to_string(),
        shell: "/bin/bash".to_string(),
        user: Some("build user".to_string()),
        interactive: true,
    };

    assert_eq!(
            docker_exec_host_command(&container, "printf '%s' \"$HOME\"", false),
            "'/opt/docker cli' 'exec' '-i' '--user' 'build user' 'dev container' '/bin/bash' '-lc' 'printf '\\''%s'\\'' \"$HOME\"'"
        );
}

#[test]
#[cfg(unix)]
fn pid_file_sweep_spares_processes_that_are_still_running() {
    // The whole point of the pid file is cancellation, so removing one that
    // is still in use silently breaks interrupt/kill for a command that may
    // legitimately run for hours. Liveness, never age.
    let live = format!("/tmp/.openbitfun-exec-live-{}.pid", uuid::Uuid::new_v4());
    let dead = format!("/tmp/.openbitfun-exec-dead-{}.pid", uuid::Uuid::new_v4());
    std::fs::write(&live, std::process::id().to_string()).expect("write live pid file");
    // Reaped immediately, so its pid is guaranteed not to be running.
    let mut corpse = std::process::Command::new("true")
        .spawn()
        .expect("spawn short-lived process");
    let dead_pid = corpse.id();
    corpse.wait().expect("reap");
    std::fs::write(&dead, dead_pid.to_string()).expect("write dead pid file");

    let output = std::process::Command::new("sh")
        .args(["-lc", &stale_pid_file_sweep()])
        .output()
        .expect("run sweep");

    assert!(output.status.success());
    let live_exists = std::path::Path::new(&live).exists();
    let dead_exists = std::path::Path::new(&dead).exists();
    let _ = std::fs::remove_file(&live);
    let _ = std::fs::remove_file(&dead);
    assert!(live_exists, "a running command's pid file must survive");
    assert!(!dead_exists, "an orphaned pid file must be removed");
}

#[test]
fn stage_tagging_is_readable_without_changing_the_error_text() {
    let original = anyhow!("Jump 1 (deploy@bastion:22) connection or authentication failed")
        .context("Could not connect to the Docker host");
    let expected_display = format!("{original}");
    let expected_alternate = format!("{original:#}");

    let tagged = tag_failed_stage("jump-1", original);

    // The tag exists only for attribution. An anyhow *context* would have
    // replaced the message the user sees with "connection stage 'jump-1'".
    assert_eq!(failed_stage_of(&tagged), Some("jump-1"));
    assert_eq!(format!("{tagged}"), expected_display);
    assert_eq!(format!("{tagged:#}"), expected_alternate);

    // Still findable once a caller layers its own context on top.
    let wrapped = tagged.context("Workspace connection failed");
    assert_eq!(failed_stage_of(&wrapped), Some("jump-1"));
    assert_eq!(format!("{wrapped}"), "Workspace connection failed");
}

#[test]
fn supervised_container_command_tracks_and_signals_the_container_process_group() {
    let container = ContainerWorkspaceConfig {
        name: "dev".to_string(),
        access: ContainerAccess::DockerExec,
        local: true,
        docker_path: "docker".to_string(),
        shell: "/bin/bash".to_string(),
        user: None,
        interactive: true,
    };
    let (wrapped, pid_file) = supervised_container_command(&container, "printf '路径'; sleep 30");
    let signal =
        container_signal_command(&pid_file, crate::remote_ssh::WorkspaceProcessSignal::Kill);

    assert!(pid_file.starts_with("/tmp/.openbitfun-exec-"));
    assert!(wrapped.contains("setsid '/bin/bash' -lc"));
    assert!(wrapped.contains("exec 9<&0 || exit 1"));
    assert!(wrapped.contains("<&9 &"));
    assert!(wrapped.contains("exec 9<&-"));
    assert!(wrapped.contains("|| tracking=0"));
    assert!(wrapped.contains("printf '%s' \"$child\" > \"$pid_file\""));
    assert!(signal.contains("[ -s \"$pid_file\" ] || exit 75"));
    assert!(signal.contains("kill -KILL -- \"-$pid\""));
    assert!(signal.contains("kill -KILL \"$pid\""));
}

#[test]
#[cfg(unix)]
fn supervised_container_command_preserves_streamed_stdin() {
    use std::io::Write;

    let container = ContainerWorkspaceConfig {
        name: "dev".to_string(),
        access: ContainerAccess::DockerExec,
        local: true,
        docker_path: "docker".to_string(),
        shell: "/bin/sh".to_string(),
        user: None,
        interactive: true,
    };
    let wrapped = supervised_container_command_with_pid_file(
        &container,
        "read value; printf 'stdin:%s' \"$value\"",
        "/tmp/.openbitfun-exec-stdin-contract.pid",
    );
    let mut child = std::process::Command::new("sh")
        .args(["-lc", &wrapped])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn supervised command");
    child
        .stdin
        .take()
        .expect("supervisor stdin")
        .write_all(b"transport-contract\n")
        .expect("write supervisor stdin");
    let output = child
        .wait_with_output()
        .expect("wait for supervised command");

    assert!(output.status.success());
    assert_eq!(output.stdout, b"stdin:transport-contract");
    assert!(
        String::from_utf8_lossy(&output.stderr).trim().is_empty(),
        "stdin forwarding must not add command stderr"
    );
}

#[test]
#[cfg(unix)]
fn supervised_container_command_keeps_working_without_a_writable_pid_location() {
    let container = ContainerWorkspaceConfig {
        name: "dev".to_string(),
        access: ContainerAccess::DockerExec,
        local: true,
        docker_path: "docker".to_string(),
        shell: "/bin/sh".to_string(),
        user: None,
        interactive: true,
    };
    let wrapped = supervised_container_command_with_pid_file(
        &container,
        "printf 'compatible'",
        "/dev/null/openbitfun-exec.pid",
    );
    let output = std::process::Command::new("sh")
        .args(["-lc", &wrapped])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, b"compatible");
    assert!(
        String::from_utf8_lossy(&output.stderr).trim().is_empty(),
        "process-control fallback must not add command stderr"
    );
}

#[tokio::test]
#[ignore = "requires OPENBITFUN_TEST_DOCKER_CONTAINER to name a running container"]
async fn local_docker_workspace_round_trip() {
    let Ok(container_name) = std::env::var("OPENBITFUN_TEST_DOCKER_CONTAINER") else {
        return;
    };
    let dir = test_data_dir("local-docker-round-trip");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let manager = SSHConnectionManager::new(dir.clone());
    let connection_id = "docker-local-contract";

    manager
        .connect(SSHConnectionConfig {
            id: connection_id.to_string(),
            name: container_name.clone(),
            host: String::new(),
            port: 22,
            username: String::new(),
            auth: SSHAuthMethod::PrivateKey {
                key_path: String::new(),
                passphrase: None,
                certificate_path: None,
            },
            default_workspace: Some("/tmp/openbitfun-remote-workspace".to_string()),
            proxy_jump: None,
            proxy_command: None,
            container: Some(ContainerWorkspaceConfig {
                name: container_name,
                access: ContainerAccess::DockerExec,
                local: true,
                docker_path: "docker".to_string(),
                shell: "/bin/sh".to_string(),
                user: None,
                interactive: true,
            }),
            wsl: None,
            options: Default::default(),
        })
        .await
        .unwrap();

    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let transport = manager
                .open_workspace_stdio(
                    connection_id,
                    "read value; printf 'stdout:%s' \"$value\"; printf 'stderr:%s' \"$value\" >&2; exit 7",
                )
                .await
                .unwrap();
        let (mut stdin, mut stdout, mut stderr, _control, completion) = transport.into_parts();
        stdin.write_all(b"transport-contract\n").await.unwrap();
        stdin.shutdown().await.unwrap();
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        stdout.read_to_end(&mut stdout_bytes).await.unwrap();
        stderr.read_to_end(&mut stderr_bytes).await.unwrap();
        let exit = completion.wait().await;
        assert_eq!(stdout_bytes, b"stdout:transport-contract");
        assert_eq!(stderr_bytes, b"stderr:transport-contract");
        assert_eq!(exit.exit_code, Some(7));
    }

    manager
        .container_mkdir(connection_id, "/tmp/openbitfun-remote-workspace", true)
        .await
        .unwrap();
    let cancellation = tokio_util::sync::CancellationToken::new();
    let cancel_after_start = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel_after_start.cancel();
    });
    let cancelled_command = manager
        .execute_command_with_options(
            connection_id,
            "trap '' INT; sleep 30; touch /tmp/openbitfun-remote-workspace/cancel-leaked",
            SSHCommandOptions {
                timeout_ms: Some(5_000),
                cancellation_token: Some(cancellation),
            },
        )
        .await
        .unwrap();
    assert!(cancelled_command.interrupted);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !manager
            .container_exists(
                connection_id,
                "/tmp/openbitfun-remote-workspace/cancel-leaked"
            )
            .await
            .unwrap(),
        "cancelled Docker command must not continue inside the container"
    );
    let original = b"OpenBitFun container workspace\0binary";
    manager
        .container_write_file(
            connection_id,
            "/tmp/openbitfun-remote-workspace/source.bin",
            original,
        )
        .await
        .unwrap();
    assert_eq!(
        manager
            .container_read_file(connection_id, "/tmp/openbitfun-remote-workspace/source.bin")
            .await
            .unwrap(),
        original
    );
    manager
        .container_write_file(
            connection_id,
            "/tmp/openbitfun-remote-workspace/atomic.bin",
            original,
        )
        .await
        .unwrap();
    let replacement = vec![b'x'; 600_000];
    let cancelled = manager
        .container_write_file_with_progress(
            connection_id,
            "/tmp/openbitfun-remote-workspace/atomic.bin",
            &replacement,
            &mut |written, _| written < 262_144,
        )
        .await;
    assert!(cancelled.is_err());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        manager
            .container_read_file(connection_id, "/tmp/openbitfun-remote-workspace/atomic.bin")
            .await
            .unwrap(),
        original,
        "cancelled upload must not replace the existing destination"
    );
    manager
        .container_rename(
            connection_id,
            "/tmp/openbitfun-remote-workspace/source.bin",
            "/tmp/openbitfun-remote-workspace/renamed.bin",
        )
        .await
        .unwrap();
    let entries = manager
        .container_read_dir(connection_id, "/tmp/openbitfun-remote-workspace")
        .await
        .unwrap();
    assert!(entries.iter().any(|entry| entry.name == "renamed.bin"));
    assert_eq!(
        manager
            .container_stat(
                connection_id,
                "/tmp/openbitfun-remote-workspace/renamed.bin"
            )
            .await
            .unwrap()
            .and_then(|entry| entry.size),
        Some(original.len() as u64)
    );
    manager
        .container_remove(
            connection_id,
            "/tmp/openbitfun-remote-workspace/renamed.bin",
            false,
        )
        .await
        .unwrap();
    assert!(!manager
        .container_exists(
            connection_id,
            "/tmp/openbitfun-remote-workspace/renamed.bin"
        )
        .await
        .unwrap());

    manager.disconnect(connection_id).await.unwrap();
    let _ = tokio::fs::remove_dir_all(&dir).await;
}
