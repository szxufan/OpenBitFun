use super::completion::{exec_command_local_completion, exec_command_remote_completion};
use super::progress::ExecOutputProgressBridge;
use crate::agentic::tools::framework::{Tool, ToolResult, ToolUseContext, ValidationResult};
use crate::util::errors::{OpenBitFunError, OpenBitFunResult};
use async_trait::async_trait;
use openbitfun_runtime_ports::{PortErrorKind, RemoteWriteStdinRequest, TerminalWriteStdinRequest};
use serde_json::{json, Value};
use tool_runtime::exec_command::{
    render_write_stdin_response_for_assistant, write_stdin_input_from_input,
    write_stdin_input_validation_message, write_stdin_result_value,
    write_stdin_session_not_found_result, ExecCommandResultFields, EXEC_TOOL_MAX_OUTPUT_CHARS,
};

pub struct WriteStdinTool;

impl Default for WriteStdinTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteStdinTool {
    pub fn new() -> Self {
        Self
    }

    fn response_for_assistant(data: &Value) -> String {
        render_write_stdin_response_for_assistant(data)
    }

    fn session_not_found_result(session_id: i32, remote: bool) -> Vec<ToolResult> {
        let result = write_stdin_session_not_found_result(session_id, remote);

        vec![ToolResult::Result {
            data: result.data,
            result_for_assistant: Some(result.assistant_message),
            image_attachments: None,
        }]
    }

    async fn call_remote_pipe(
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> OpenBitFunResult<Vec<ToolResult>> {
        let parsed_input = write_stdin_input_from_input(input).ok_or_else(|| {
            OpenBitFunError::tool("session_id is required for WriteStdin".to_string())
        })?;
        let session_id = parsed_input.session_id;
        let request = RemoteWriteStdinRequest {
            session_id,
            chars: parsed_input.chars,
            append_enter: parsed_input.append_enter,
            yield_time_ms: Some(parsed_input.yield_time_ms),
            max_output_chars: Some(EXEC_TOOL_MAX_OUTPUT_CHARS),
            wait_interrupt: context.round_injection_preemption_token().cloned(),
        };
        let remote_exec_port = context.remote_exec_port().ok_or_else(|| {
            OpenBitFunError::tool(
                "remote exec runtime service is required for WriteStdin".to_string(),
            )
        })?;
        let progress_bridge = ExecOutputProgressBridge::start(context, self.name());
        let response_result = if let Some(bridge) = progress_bridge.as_ref() {
            remote_exec_port
                .write_stdin_streaming(request, bridge.sender())
                .await
        } else {
            remote_exec_port.write_stdin(request).await
        };
        if let Some(bridge) = progress_bridge {
            bridge.finish().await;
        }
        let response = match response_result {
            Ok(response) => response,
            Err(error) if error.kind == PortErrorKind::NotFound => {
                return Ok(Self::session_not_found_result(session_id, true));
            }
            Err(error) => {
                return Err(OpenBitFunError::tool(format!(
                    "WriteStdin failed: {}",
                    error.message
                )));
            }
        };

        let mut data = write_stdin_result_value(ExecCommandResultFields {
            chunk_id: response.chunk_id,
            wall_time_seconds: response.wall_time_seconds,
            output: response.output,
            session_id: response.session_id,
            exit_code: response.exit_code,
            original_output_chars: response.original_output_chars,
            completion: response.completion.map(exec_command_remote_completion),
            remote: true,
        });
        if response.wait_interrupted {
            data["wait_end_reason"] = json!("steered");
        }
        let result_for_assistant = Self::response_for_assistant(&data);

        Ok(vec![ToolResult::Result {
            data,
            result_for_assistant: Some(result_for_assistant),
            image_attachments: None,
        }])
    }
}

#[async_trait]
impl Tool for WriteStdinTool {
    fn name(&self) -> &str {
        "WriteStdin"
    }

    async fn description(&self) -> OpenBitFunResult<String> {
        Ok(r#"Writes stdin to, or polls, a running ExecCommand session.

Pass the session_id returned by ExecCommand. Leave chars empty or omit it to poll for new output.
When waiting for a long-running command to finish, explicitly set a longer yield_time_ms, such as 600000 ms, to avoid frequent polling. Use shorter waits when you need to respond to interactive prompts or inspect intermediate output.
User steering can end the wait early and return output collected so far. The process continues running; already submitted input is not undone. Use the returned session_id to poll again without resending input.
chars is sent only to sessions started with tty=true. For tty=false sessions, this tool only polls.
Use append_enter=true to submit a line after chars. Use this for line-oriented interactive prompts instead of trying to encode \\r or \\n manually.
Output is only what was produced during this tool call's wait window."#
            .to_string())
    }

    fn short_description(&self) -> String {
        "Write to or poll a running ExecCommand session.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "number",
                    "description": "session_id returned by ExecCommand while a process is still running."
                },
                "chars": {
                    "type": "string",
                    "description": "Characters to write to stdin. Empty or omitted means poll for new output."
                },
                "append_enter": {
                    "type": "boolean",
                    "description": "When true, append an Enter key after chars."
                },
                "yield_time_ms": {
                    "type": "number",
                    "description": "How long to wait for output before yielding, in milliseconds. Defaults to 180000 ms. When waiting for a long-running command to finish, explicitly set a longer wait, such as 600000 ms, to avoid frequent polling."
                }
            },
            "required": ["session_id"],
            "additionalProperties": false
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "wall_time_seconds": {"type": "number"},
                "output": {"type": "string"},
                "session_id": {"anyOf": [{"type": "integer"}, {"type": "null"}]},
                "exit_code": {"anyOf": [{"type": "integer"}, {"type": "null"}]},
                "wait_end_reason": {"type": "string", "enum": ["steered"]},
                "completion": {"anyOf": [{"type": "object", "properties": {"source": {"type": "string"}, "status": {"type": "string"}}, "additionalProperties": false}, {"type": "null"}]}
            },
            "required": ["wall_time_seconds", "output", "session_id", "exit_code", "completion"],
            "additionalProperties": false
        })
    }

    fn is_readonly(&self) -> bool {
        false
    }

    fn is_concurrency_safe(&self, _input: Option<&Value>) -> bool {
        false
    }

    fn manages_own_execution_timeout(&self) -> bool {
        true
    }

    fn round_injection_yieldable(&self) -> bool {
        true
    }

    async fn validate_input(
        &self,
        input: &Value,
        context: Option<&ToolUseContext>,
    ) -> ValidationResult {
        if let Some(message) = write_stdin_input_validation_message(input) {
            return ValidationResult {
                result: false,
                message: Some(message.to_string()),
                error_code: Some(400),
                meta: None,
            };
        }
        if let (Some(context), Some(chars)) = (context, input.get("chars").and_then(Value::as_str))
        {
            if let Some(rejection) =
                crate::agentic::execution::edit_constraint_guard::check_bash_command(context, chars)
                    .await
            {
                return rejection;
            }
        }
        ValidationResult {
            result: true,
            message: None,
            error_code: None,
            meta: None,
        }
    }

    async fn validate_input_rewrite_invariants(
        &self,
        input: &Value,
        context: Option<&ToolUseContext>,
    ) -> ValidationResult {
        let Some((context, chars)) = context.zip(input.get("chars").and_then(Value::as_str)) else {
            return ValidationResult::default();
        };
        crate::agentic::execution::edit_constraint_guard::check_bash_command(context, chars)
            .await
            .unwrap_or_default()
    }

    async fn call_impl(
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> OpenBitFunResult<Vec<ToolResult>> {
        if context.is_remote() {
            return self.call_remote_pipe(input, context).await;
        }

        let parsed_input = write_stdin_input_from_input(input).ok_or_else(|| {
            OpenBitFunError::tool("session_id is required for WriteStdin".to_string())
        })?;
        let session_id = parsed_input.session_id;
        let request = TerminalWriteStdinRequest {
            session_id,
            chars: parsed_input.chars,
            append_enter: parsed_input.append_enter,
            yield_time_ms: Some(parsed_input.yield_time_ms),
            max_output_chars: Some(EXEC_TOOL_MAX_OUTPUT_CHARS),
            wait_interrupt: context.round_injection_preemption_token().cloned(),
        };
        let terminal_port = context.terminal_port().ok_or_else(|| {
            OpenBitFunError::tool("terminal runtime service is required for WriteStdin".to_string())
        })?;
        let progress_bridge = ExecOutputProgressBridge::start(context, self.name());
        let response_result = if let Some(bridge) = progress_bridge.as_ref() {
            terminal_port
                .write_stdin_streaming(request, bridge.sender())
                .await
        } else {
            terminal_port.write_stdin(request).await
        };
        if let Some(bridge) = progress_bridge {
            bridge.finish().await;
        }
        let response = match response_result {
            Ok(response) => response,
            Err(error) if error.kind == PortErrorKind::NotFound => {
                return Ok(Self::session_not_found_result(session_id, false));
            }
            Err(error) => {
                return Err(OpenBitFunError::tool(format!(
                    "WriteStdin failed: {}",
                    error.message
                )));
            }
        };

        let mut data = write_stdin_result_value(ExecCommandResultFields {
            chunk_id: response.chunk_id,
            wall_time_seconds: response.wall_time_seconds,
            output: response.output,
            session_id: response.session_id,
            exit_code: response.exit_code,
            original_output_chars: response.original_output_chars,
            completion: response.completion.map(exec_command_local_completion),
            remote: false,
        });
        if response.wait_interrupted {
            data["wait_end_reason"] = json!("steered");
        }
        let result_for_assistant = Self::response_for_assistant(&data);

        Ok(vec![ToolResult::Result {
            data,
            result_for_assistant: Some(result_for_assistant),
            image_attachments: None,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::WriteStdinTool;
    use crate::agentic::tools::framework::ToolResult;

    #[test]
    fn session_not_found_result_uses_plain_assistant_message() {
        let results = WriteStdinTool::session_not_found_result(123, false);
        let ToolResult::Result {
            data,
            result_for_assistant,
            ..
        } = &results[0]
        else {
            panic!("expected result");
        };

        assert_eq!(
            data.get("status").and_then(|value| value.as_str()),
            Some("session_not_found")
        );
        assert_eq!(
            data.get("requested_session_id")
                .and_then(|value| value.as_i64()),
            Some(123)
        );
        let assistant = result_for_assistant.as_deref().expect("assistant text");
        assert!(assistant.contains("ExecCommand session 123 was not found"));
        assert!(!assistant.contains("<wall_time>"));
        assert!(!assistant.contains("<output>"));
    }
}

#[cfg(test)]
mod steering_tests {
    use super::*;
    use openbitfun_runtime_ports::*;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn write_stdin_owns_timeout_and_yields_to_round_injection() {
        let tool = WriteStdinTool::new();
        assert!(tool.manages_own_execution_timeout());
        assert!(tool.round_injection_yieldable());
        assert!(!tool.is_concurrency_safe(None));
    }

    #[derive(Debug)]
    struct TerminalProbe {
        streaming: bool,
    }

    impl RuntimeServicePort for TerminalProbe {
        fn capability(&self) -> RuntimeServiceCapability {
            RuntimeServiceCapability::Terminal
        }
    }

    #[async_trait]
    impl TerminalPort for TerminalProbe {
        async fn exec_command(
            &self,
            _: TerminalExecCommandRequest,
        ) -> PortResult<TerminalExecCommandResponse> {
            panic!("unexpected exec")
        }
        async fn exec_command_streaming(
            &self,
            _: TerminalExecCommandRequest,
            _: TerminalExecStreamingOutputSink,
        ) -> PortResult<TerminalExecCommandResponse> {
            panic!("unexpected exec")
        }
        async fn send_stdin(&self, _: TerminalSendStdinRequest) -> PortResult<()> {
            panic!("input must not be replayed")
        }
        async fn control_session(
            &self,
            _: TerminalExecControlRequest,
        ) -> PortResult<TerminalExecCommandResponse> {
            panic!("steering must not control the process")
        }

        async fn write_stdin(
            &self,
            request: TerminalWriteStdinRequest,
        ) -> PortResult<TerminalExecCommandResponse> {
            assert!(!self.streaming);
            Ok(self.response(request))
        }

        async fn write_stdin_streaming(
            &self,
            request: TerminalWriteStdinRequest,
            sink: TerminalExecStreamingOutputSink,
        ) -> PortResult<TerminalExecCommandResponse> {
            assert!(self.streaming);
            sink.send("partial".into()).await.unwrap();
            Ok(self.response(request))
        }
    }

    impl TerminalProbe {
        fn response(&self, request: TerminalWriteStdinRequest) -> TerminalExecCommandResponse {
            assert_eq!(request.session_id, 42);
            assert_eq!(request.chars, "answer");
            assert!(request.append_enter);
            assert_eq!(request.yield_time_ms, Some(600_000));
            let wait_interrupted = request
                .wait_interrupt
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled);
            TerminalExecCommandResponse {
                chunk_id: "chunk".into(),
                wall_time_seconds: 0.01,
                output: "partial".into(),
                session_id: Some(42),
                exit_code: None,
                original_output_chars: 7,
                completion: None,
                wait_interrupted,
            }
        }
    }

    #[derive(Debug)]
    struct RemoteProbe {
        streaming: bool,
    }

    impl RuntimeServicePort for RemoteProbe {
        fn capability(&self) -> RuntimeServiceCapability {
            RuntimeServiceCapability::RemoteExec
        }
    }

    #[async_trait]
    impl RemoteExecPort for RemoteProbe {
        async fn exec_command_once(
            &self,
            _: RemoteExecOneShotCommandRequest,
        ) -> PortResult<RemoteExecOneShotCommandResponse> {
            panic!("unexpected exec")
        }
        async fn exec_command(
            &self,
            _: RemoteExecCommandRequest,
        ) -> PortResult<RemoteExecCommandResponse> {
            panic!("unexpected exec")
        }
        async fn exec_command_streaming(
            &self,
            _: RemoteExecCommandRequest,
            _: RemoteExecStreamingOutputSink,
        ) -> PortResult<RemoteExecCommandResponse> {
            panic!("unexpected exec")
        }
        async fn send_stdin(&self, _: RemoteSendStdinRequest) -> PortResult<()> {
            panic!("input must not be replayed")
        }
        async fn control_session(
            &self,
            _: RemoteExecControlRequest,
        ) -> PortResult<RemoteExecCommandResponse> {
            panic!("steering must not control the process")
        }

        async fn write_stdin(
            &self,
            request: RemoteWriteStdinRequest,
        ) -> PortResult<RemoteExecCommandResponse> {
            assert!(!self.streaming);
            Ok(self.response(request))
        }

        async fn write_stdin_streaming(
            &self,
            request: RemoteWriteStdinRequest,
            sink: RemoteExecStreamingOutputSink,
        ) -> PortResult<RemoteExecCommandResponse> {
            assert!(self.streaming);
            sink.send("partial".into()).await.unwrap();
            Ok(self.response(request))
        }
    }

    impl RemoteProbe {
        fn response(&self, request: RemoteWriteStdinRequest) -> RemoteExecCommandResponse {
            assert_eq!(request.session_id, 42);
            assert_eq!(request.chars, "answer");
            assert!(request.append_enter);
            assert_eq!(request.yield_time_ms, Some(600_000));
            let wait_interrupted = request
                .wait_interrupt
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled);
            RemoteExecCommandResponse {
                chunk_id: "chunk".into(),
                wall_time_seconds: 0.01,
                output: "partial".into(),
                session_id: Some(42),
                exit_code: None,
                original_output_chars: 7,
                completion: None,
                wait_interrupted,
            }
        }
    }

    #[tokio::test]
    async fn write_stdin_steering_reaches_both_ports_and_preserves_legacy_results() {
        for remote in [false, true] {
            for streaming in [false, true] {
                for steered in [false, true] {
                    let mut context = ToolUseContext::for_tool_listing(None, None);
                    context.tool_call_id = streaming.then(|| "stdin-test".to_string());
                    let token = CancellationToken::new();
                    token.cancel();
                    context.runtime_handles = context
                        .runtime_handles
                        .with_round_injection_preemption_token(steered.then_some(token))
                        .with_terminal_port(Some(Arc::new(TerminalProbe { streaming })))
                        .with_remote_exec_port(Some(Arc::new(RemoteProbe { streaming })));
                    let input = json!({"session_id":42, "chars":"answer", "append_enter":true, "yield_time_ms":600_000});
                    let tool = WriteStdinTool::new();
                    let results = if remote {
                        tool.call_remote_pipe(&input, &context).await
                    } else {
                        tool.call_impl(&input, &context).await
                    }
                    .unwrap();
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
                    if steered {
                        assert_eq!(data["wait_end_reason"], "steered");
                        assert!(result_for_assistant
                            .as_ref()
                            .unwrap()
                            .contains("user steering arrived"));
                    } else {
                        assert!(data.get("wait_end_reason").is_none());
                        assert!(!result_for_assistant.as_ref().unwrap().contains("steering"));
                    }
                    // Tool results remain ordinary JSON, including older payloads
                    // which have no wait_end_reason field.
                    let restored: Value =
                        serde_json::from_str(&serde_json::to_string(data).unwrap()).unwrap();
                    assert_eq!(&restored, data);
                }
            }
        }
    }
}
