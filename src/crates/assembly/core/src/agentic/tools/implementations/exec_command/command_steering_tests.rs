use super::tests::{local_tool_context, remote_tool_context};
use super::*;
use crate::agentic::tools::framework::ToolResult;
use openbitfun_runtime_ports::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct ExecProbe {
    streaming: bool,
    calls: AtomicUsize,
}

impl RuntimeServicePort for ExecProbe {
    fn capability(&self) -> RuntimeServiceCapability {
        RuntimeServiceCapability::Terminal
    }
}

#[async_trait::async_trait]
impl TerminalPort for ExecProbe {
    async fn exec_command(
        &self,
        request: TerminalExecCommandRequest,
    ) -> PortResult<TerminalExecCommandResponse> {
        assert_eq!(self.streaming, false);
        assert!(request.tty);
        assert_eq!(request.yield_time_ms, Some(600_000));
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(request.argv.last().unwrap().contains("echo steering-probe"));
        Ok(TerminalExecCommandResponse {
            chunk_id: "chunk".into(),
            wall_time_seconds: 0.01,
            output: "partial".into(),
            session_id: Some(42),
            exit_code: None,
            original_output_chars: 7,
            completion: None,
            wait_interrupted: request
                .wait_interrupt
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled),
        })
    }
    async fn exec_command_streaming(
        &self,
        request: TerminalExecCommandRequest,
        sink: TerminalExecStreamingOutputSink,
    ) -> PortResult<TerminalExecCommandResponse> {
        assert_eq!(self.streaming, true);
        assert!(request.tty);
        assert_eq!(request.yield_time_ms, Some(600_000));
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(request.argv.last().unwrap().contains("echo steering-probe"));
        sink.send("partial".into()).await.unwrap();
        Ok(TerminalExecCommandResponse {
            chunk_id: "chunk".into(),
            wall_time_seconds: 0.01,
            output: "partial".into(),
            session_id: Some(42),
            exit_code: None,
            original_output_chars: 7,
            completion: None,
            wait_interrupted: request
                .wait_interrupt
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled),
        })
    }
    async fn write_stdin(
        &self,
        _: TerminalWriteStdinRequest,
    ) -> PortResult<TerminalExecCommandResponse> {
        panic!("initial wait must not replay input or control the process")
    }
    async fn write_stdin_streaming(
        &self,
        _: TerminalWriteStdinRequest,
        _: TerminalExecStreamingOutputSink,
    ) -> PortResult<TerminalExecCommandResponse> {
        panic!("initial wait must not replay input or control the process")
    }
    async fn send_stdin(&self, _: TerminalSendStdinRequest) -> PortResult<()> {
        panic!("initial wait must not replay input or control the process")
    }
    async fn control_session(
        &self,
        _: TerminalExecControlRequest,
    ) -> PortResult<TerminalExecCommandResponse> {
        panic!("initial wait must not replay input or control the process")
    }
}

#[async_trait::async_trait]
impl RemoteExecPort for ExecProbe {
    async fn exec_command_once(
        &self,
        _: RemoteExecOneShotCommandRequest,
    ) -> PortResult<RemoteExecOneShotCommandResponse> {
        Ok(RemoteExecOneShotCommandResponse {
            stdout: "/bin/sh\n".into(),
            stderr: String::new(),
            exit_code: 0,
            interrupted: false,
            timed_out: false,
        })
    }
    async fn exec_command(
        &self,
        request: RemoteExecCommandRequest,
    ) -> PortResult<RemoteExecCommandResponse> {
        // Environment capture is a separate best-effort setup command. The
        // fork also caps the main exec's output, so identify the snapshot by
        // its command marker rather than by max_output_chars.
        if request
            .command
            .contains("__OPENBITFUN_REMOTE_ENV_SNAPSHOT_BEGIN__")
        {
            assert!(request.wait_interrupt.is_none());
            return Err(PortError::new(
                PortErrorKind::NotAvailable,
                "no environment snapshot",
            ));
        }
        assert_eq!(self.streaming, false);
        assert!(request.tty);
        assert_eq!(request.yield_time_ms, Some(600_000));
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(request.command.contains("echo steering-probe"));
        Ok(RemoteExecCommandResponse {
            chunk_id: "chunk".into(),
            wall_time_seconds: 0.01,
            output: "partial".into(),
            session_id: Some(42),
            exit_code: None,
            original_output_chars: 7,
            completion: None,
            wait_interrupted: request
                .wait_interrupt
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled),
        })
    }
    async fn exec_command_streaming(
        &self,
        request: RemoteExecCommandRequest,
        sink: RemoteExecStreamingOutputSink,
    ) -> PortResult<RemoteExecCommandResponse> {
        assert_eq!(self.streaming, true);
        assert!(request.tty);
        assert_eq!(request.yield_time_ms, Some(600_000));
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(request.command.contains("echo steering-probe"));
        sink.send("partial".into()).await.unwrap();
        Ok(RemoteExecCommandResponse {
            chunk_id: "chunk".into(),
            wall_time_seconds: 0.01,
            output: "partial".into(),
            session_id: Some(42),
            exit_code: None,
            original_output_chars: 7,
            completion: None,
            wait_interrupted: request
                .wait_interrupt
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled),
        })
    }
    async fn write_stdin(
        &self,
        _: RemoteWriteStdinRequest,
    ) -> PortResult<RemoteExecCommandResponse> {
        panic!("initial wait must not replay input or control the process")
    }
    async fn write_stdin_streaming(
        &self,
        _: RemoteWriteStdinRequest,
        _: RemoteExecStreamingOutputSink,
    ) -> PortResult<RemoteExecCommandResponse> {
        panic!("initial wait must not replay input or control the process")
    }
    async fn send_stdin(&self, _: RemoteSendStdinRequest) -> PortResult<()> {
        panic!("initial wait must not replay input or control the process")
    }
    async fn control_session(
        &self,
        _: RemoteExecControlRequest,
    ) -> PortResult<RemoteExecCommandResponse> {
        panic!("initial wait must not replay input or control the process")
    }
}

#[test]
fn exec_command_owns_timeout_and_yields_to_round_injection() {
    let tool = ExecCommandTool::new();
    assert!(tool.manages_own_execution_timeout());
    assert!(tool.round_injection_yieldable());
    assert!(tool.is_concurrency_safe(None));
    assert!(tool.output_schema()["properties"]
        .get("wait_end_reason")
        .is_some());
    assert!(!tool.output_schema()["required"]
        .as_array()
        .unwrap()
        .contains(&json!("wait_end_reason")));
}

#[tokio::test]
async fn exec_command_steering_reaches_both_ports_and_preserves_legacy_results() {
    let workspace = tempfile::tempdir().unwrap();
    for remote in [false, true] {
        for streaming in [false, true] {
            for steered in [false, true] {
                let mut context = if remote {
                    remote_tool_context("/home/me/project")
                } else {
                    local_tool_context(workspace.path())
                };
                context.tool_call_id =
                    streaming.then(|| format!("exec-steering-{remote}-{steered}"));
                let probe = Arc::new(ExecProbe {
                    streaming,
                    calls: AtomicUsize::new(0),
                });
                let token = CancellationToken::new();
                token.cancel();
                context.runtime_handles = context
                    .runtime_handles
                    .with_round_injection_preemption_token(steered.then_some(token))
                    .with_terminal_port(Some(probe.clone()))
                    .with_remote_exec_port(Some(probe.clone()));
                let tool = ExecCommandTool::new();
                let results = tool
                    .call_impl(
                        &json!({
                            "cmd":"echo steering-probe", "tty":true, "yield_time_ms":600_000,
                        }),
                        &context,
                    )
                    .await
                    .unwrap();
                assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
                let ToolResult::Result {
                    data,
                    result_for_assistant,
                    ..
                } = &results[0]
                else {
                    panic!("steering must return a normal result");
                };
                assert_eq!(data["output"], "partial");
                assert_eq!(data["session_id"], 42);
                assert!(data["exit_code"].is_null());
                assert!(data["completion"].is_null());
                let assistant = result_for_assistant.as_deref().unwrap();
                if steered {
                    assert_eq!(data["wait_end_reason"], "steered");
                    assert!(assistant.contains("user steering arrived"));
                    assert!(assistant.contains("do not launch the command again"));
                } else {
                    assert!(data.get("wait_end_reason").is_none());
                    assert!(!assistant.contains("user steering arrived"));
                }
                let restored: serde_json::Value =
                    serde_json::from_str(&serde_json::to_string(data).unwrap()).unwrap();
                assert_eq!(&restored, data);
            }
        }
    }
}
