use super::background_command_output::{
    background_command_output_capture, background_command_output_retention_policy_message,
    BackgroundCommandOutputStatus, ListBackgroundCommandOutputRequest,
    ReadBackgroundCommandOutputRefRequest,
};
use crate::agentic::tools::framework::{Tool, ToolResult, ToolUseContext, ValidationResult};
use crate::util::errors::{OpenBitFunError, OpenBitFunResult};
use async_trait::async_trait;
use openbitfun_runtime_ports::ExecOutputStream;
use serde_json::{json, Value};
use tool_runtime::output_read::{select_output_lines, ExecOutputReadOptions};

/// Read-only access to the output that ExecCommand already captured. Reading
/// starts nothing and mutates nothing, so this tool is auto-approved and safe
/// to call freely while polling long-running commands.
pub struct ExecReadOutputTool;

impl Default for ExecReadOutputTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecReadOutputTool {
    pub fn new() -> Self {
        Self
    }

    fn structured_result(data: Value, assistant_message: String) -> Vec<ToolResult> {
        vec![ToolResult::Result {
            data,
            result_for_assistant: Some(assistant_message),
            image_attachments: None,
        }]
    }

    fn not_found_result(handle: &str) -> Vec<ToolResult> {
        let message = format!(
            "No captured output found for '{handle}'. It may have never existed, or it was collected after retention expired. {}",
            background_command_output_retention_policy_message()
        );
        Self::structured_result(
            json!({
                "status": "output_not_found",
                "message": message,
                "handle": handle,
            }),
            message,
        )
    }

    fn error_result(message: String) -> Vec<ToolResult> {
        Self::structured_result(
            json!({ "status": "error", "message": message }),
            format!("ExecReadOutput failed: {message}"),
        )
    }

    fn terminal_status(status: &BackgroundCommandOutputStatus) -> bool {
        !matches!(status, BackgroundCommandOutputStatus::Running)
    }

    fn stream_from_input(input: &Value) -> Result<Option<ExecOutputStream>, String> {
        match input.get("stream").and_then(Value::as_str) {
            None => Ok(None),
            Some("combined") => Ok(Some(ExecOutputStream::Combined)),
            Some("stdout") => Ok(Some(ExecOutputStream::Stdout)),
            Some("stderr") => Ok(Some(ExecOutputStream::Stderr)),
            Some(other) => Err(format!(
                "stream must be one of 'combined', 'stdout', or 'stderr' (got '{other}')"
            )),
        }
    }

    fn line_options_from_input(input: &Value) -> Result<ExecOutputReadOptions, String> {
        let positive = |key: &str| -> Result<Option<usize>, String> {
            match input.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(value) => value
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .filter(|value| *value >= 1)
                    .map(Some)
                    .ok_or_else(|| format!("{key} must be a positive integer")),
            }
        };
        Ok(ExecOutputReadOptions {
            filter: input
                .get("filter")
                .and_then(Value::as_str)
                .map(str::to_string),
            stream: None,
            head: positive("head")?,
            tail: positive("tail")?,
            from_line: positive("from_line")?,
            to_line: positive("to_line")?,
            max_lines: positive("max_lines")?,
        })
    }

    async fn call_read(
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> OpenBitFunResult<Vec<ToolResult>> {
        let stream = match Self::stream_from_input(input) {
            Ok(stream) => stream,
            Err(message) => return Ok(Self::error_result(message)),
        };
        let line_options = match Self::line_options_from_input(input) {
            Ok(options) => options,
            Err(message) => return Ok(Self::error_result(message)),
        };
        let cursor = input.get("cursor").and_then(Value::as_u64);
        let capture = background_command_output_capture();

        // The handle doubles as the capture id when it exists; a session id
        // resolves through the session index of still-tracked captures.
        let capture_id = if let Some(output_ref) = input
            .get("output_ref")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|output_ref| !output_ref.is_empty())
        {
            Some(output_ref.to_string())
        } else if let Some(session_id) = input.get("session_id").and_then(Value::as_i64) {
            match i32::try_from(session_id) {
                Ok(session_id) => {
                    capture
                        .capture_id_for_exec_session(context.is_remote(), session_id)
                        .await
                }
                Err(_) => {
                    return Ok(Self::error_result(format!(
                        "session_id {session_id} is out of range"
                    )))
                }
            }
        } else {
            None
        };
        let Some(capture_id) = capture_id else {
            return Ok(Self::error_result(
                "action \"read\" requires an output_ref (preferred) or a session_id of a still-tracked session.".to_string(),
            ));
        };

        // Line selection always works from the newest snapshot; incremental
        // reads use the cursor protocol.
        let effective_cursor = if line_options.has_line_selection() {
            None
        } else {
            cursor
        };
        let Some(response) = capture
            .read_by_ref(ReadBackgroundCommandOutputRefRequest {
                capture_id: capture_id.clone(),
                cursor: effective_cursor,
                stream,
            })
            .await
        else {
            return Ok(Self::not_found_result(&capture_id));
        };

        if response.metadata.tty
            && matches!(
                stream,
                Some(ExecOutputStream::Stdout) | Some(ExecOutputStream::Stderr)
            )
        {
            let message = "Stream selection is not available for tty sessions: the terminal merges stdout and stderr. Re-read without the stream option.";
            return Ok(Self::structured_result(
                json!({
                    "status": "unsupported",
                    "reason": "tty_session_combined_streams",
                    "message": message,
                    "output_ref": capture_id,
                }),
                format!("ExecReadOutput: {message}"),
            ));
        }

        let eof = Self::terminal_status(&response.metadata.status);
        let base_output = match (&response.snapshot, response.chunks.is_empty()) {
            (Some(snapshot), _) => snapshot.clone(),
            (None, false) => response.chunks.concat(),
            (None, true) => String::new(),
        };

        if line_options.has_line_selection() {
            let selection = match select_output_lines(&base_output, &line_options) {
                Ok(selection) => selection,
                Err(error) => return Ok(Self::error_result(error.message)),
            };
            let mut data = selection.to_value();
            data["status"] = json!("ok");
            data["output_ref"] = json!(capture_id);
            data["eof"] = json!(eof);
            data["command_status"] = json!(serde_json::to_value(&response.metadata.status)
                .unwrap_or_else(|_| Value::String("unknown".to_string())));
            let assistant = format!(
                "ExecReadOutput lines {}-{} of {} (has_more: {}).\n{}",
                selection.start_line,
                selection.end_line,
                selection.total_lines,
                selection.has_more,
                selection.output
            );
            return Ok(Self::structured_result(data, assistant));
        }

        let mut data = json!({
            "status": "ok",
            "output_ref": capture_id,
            "output": base_output,
            "cursor": response.cursor,
            "reset": response.reset,
            "eof": eof,
            "command_status": serde_json::to_value(&response.metadata.status)
                .unwrap_or_else(|_| Value::String("unknown".to_string())),
            "truncated_from_start": response.metadata.truncated_from_start,
        });
        if let Some(exit_code) = response.metadata.exit_code {
            data["exit_code"] = json!(exit_code);
        }
        if let Some(session_id) = response.metadata.exec_session_id {
            data["session_id"] = json!(session_id);
        }
        let assistant = format!(
            "ExecReadOutput cursor {} (eof: {eof}, status: {:?}).\n{}",
            response.cursor,
            response.metadata.status,
            data["output"].as_str().unwrap_or("")
        );
        Ok(Self::structured_result(data, assistant))
    }

    async fn call_list(&self, context: &ToolUseContext) -> OpenBitFunResult<Vec<ToolResult>> {
        let capture = background_command_output_capture();
        let activities = capture
            .list_with_capture_ids(ListBackgroundCommandOutputRequest {
                agent_session_id: context.session_id.clone(),
            })
            .await;
        let rows: Vec<Value> = activities
            .iter()
            .map(|(capture_id, metadata)| {
                json!({
                    "output_ref": capture_id,
                    "session_id": metadata.exec_session_id,
                    "command": metadata.command,
                    "status": serde_json::to_value(&metadata.status)
                        .unwrap_or_else(|_| Value::String("unknown".to_string())),
                    "exit_code": metadata.exit_code,
                    "started_at": metadata.started_at,
                    "ended_at": metadata.ended_at,
                    "tty": metadata.tty,
                    "remote": metadata.remote,
                    "retained_bytes": metadata.retained_bytes,
                    "truncated_from_start": metadata.truncated_from_start,
                })
            })
            .collect();
        let data = json!({
            "status": "ok",
            "count": rows.len(),
            "activities": rows,
        });
        let assistant = if rows.is_empty() {
            "No captured command output for this session yet.".to_string()
        } else {
            format!(
                "{} captured command(s). Each row's output_ref can be read with action \"read\".",
                rows.len()
            )
        };
        Ok(Self::structured_result(data, assistant))
    }
}

#[async_trait]
impl Tool for ExecReadOutputTool {
    fn name(&self) -> &str {
        "ExecReadOutput"
    }

    async fn description(&self) -> OpenBitFunResult<String> {
        Ok(r#"Re-reads output that ExecCommand already captured, or lists captured commands. Nothing is executed; reads are repeatable and addressable.

Actions:
- "list": returns the commands captured for this session with their output_ref, status, exit code, and retention bytes.
- "read": returns output for one capture. Address it by "output_ref" (the handle printed by ExecCommand) or by "session_id" of a still-tracked session.

Read modes (pick one):
- Incremental: pass "cursor" from the previous read; the response includes the next "cursor" and "eof". Omit "cursor" for a full snapshot. This is the low-cost way to poll running commands.
- Line selection: pass any of "filter" (regex; keeps original line numbers, grep -n style), "head", "tail", "from_line"/"to_line" (1-based inclusive range), "max_lines" (default 200, max 2000). Lines come back as "N| text" and "has_more" means continue with "from_line" = "end_line" + 1.

"stream" ("combined" default, "stdout", "stderr") filters both modes; tty sessions cannot split streams and answer with an explicit unsupported status.

Prefer this tool over piping grep/head/tail/wc/sed in the command itself: run the raw command, then inspect its retained output here. Only filter in the command when output exceeds the per-command retention cap."#
            .to_string())
    }

    fn short_description(&self) -> String {
        "Re-read or list captured ExecCommand output.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "list"],
                    "description": "\"read\" returns captured output; \"list\" returns the captured commands for this session."
                },
                "output_ref": {
                    "type": "string",
                    "description": "Capture handle printed by ExecCommand (action \"read\")."
                },
                "session_id": {
                    "type": "number",
                    "description": "ExecCommand session id of a still-tracked capture (action \"read\")."
                },
                "cursor": {
                    "type": "number",
                    "description": "Continue an incremental read from this cursor (action \"read\"). Omit for a snapshot."
                },
                "filter": {
                    "type": "string",
                    "description": "Regex; only matching lines are kept with their original line numbers (action \"read\")."
                },
                "stream": {
                    "type": "string",
                    "enum": ["combined", "stdout", "stderr"],
                    "description": "Which stream to read (action \"read\"). Default \"combined\". Not available for tty sessions."
                },
                "head": {
                    "type": "number",
                    "description": "First N lines of the filtered view (action \"read\")."
                },
                "tail": {
                    "type": "number",
                    "description": "Last N lines of the filtered view (action \"read\")."
                },
                "from_line": {
                    "type": "number",
                    "description": "Start of a 1-based inclusive line range on original line numbers (action \"read\")."
                },
                "to_line": {
                    "type": "number",
                    "description": "End of a 1-based inclusive line range on original line numbers (action \"read\")."
                },
                "max_lines": {
                    "type": "number",
                    "description": "Maximum lines per read. Default 200, max 2000 (action \"read\")."
                }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": {"type": "string"},
                "output": {"type": "string"},
                "output_ref": {"type": "string"},
                "cursor": {"type": "number"},
                "eof": {"type": "boolean"},
                "total_lines": {"type": "number"},
                "start_line": {"type": "number"},
                "end_line": {"type": "number"},
                "has_more": {"type": "boolean"},
                "activities": {"type": "array"}
            },
            "required": ["status"],
            "additionalProperties": true
        })
    }

    fn is_readonly(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: Option<&Value>) -> bool {
        true
    }

    async fn validate_input(
        &self,
        input: &Value,
        _context: Option<&ToolUseContext>,
    ) -> ValidationResult {
        let action = input.get("action").and_then(Value::as_str);
        match action {
            Some("read") | Some("list") => ValidationResult {
                result: true,
                message: None,
                error_code: None,
                meta: None,
            },
            _ => ValidationResult {
                result: false,
                message: Some("action must be either 'read' or 'list'".to_string()),
                error_code: Some(400),
                meta: None,
            },
        }
    }

    async fn call_impl(
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> OpenBitFunResult<Vec<ToolResult>> {
        match input.get("action").and_then(Value::as_str) {
            Some("read") => self.call_read(input, context).await,
            Some("list") => self.call_list(context).await,
            _ => Err(OpenBitFunError::tool(
                "action must be either 'read' or 'list'".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::background_command_output::{
        StartBackgroundCommandOutputCapture, BACKGROUND_COMMAND_OUTPUT_CAPTURE_LIMIT_BYTES,
        MAX_BACKGROUND_COMMAND_OUTPUT_RECORDS,
    };
    use super::*;
    use crate::agentic::tools::framework::Tool;
    use crate::agentic::tools::ToolRuntimeRestrictions;
    use crate::agentic::workspace::WorkspaceBinding;
    use openbitfun_runtime_ports::ExecOutputChunk;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tokio::sync::mpsc;

    fn unique_capture_id(prefix: &str) -> String {
        format!(
            "{prefix}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        )
    }

    fn result_data(results: &[ToolResult]) -> Value {
        match results.first() {
            Some(ToolResult::Result { data, .. }) => data.clone(),
            other => panic!("expected a tool result, got {other:?}"),
        }
    }

    fn tool_context(session_id: Option<&str>) -> ToolUseContext {
        ToolUseContext {
            tool_call_id: None,
            agent_type: Some("Standard".to_string()),
            session_id: session_id.map(str::to_string),
            dialog_turn_id: None,
            workspace: Some(WorkspaceBinding::new(
                None,
                PathBuf::from(".")
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from(".")),
            )),
            loaded_deferred_tool_specs: Vec::new(),
            primary_model_facts: tool_runtime::context::PrimaryModelFacts::default(),
            custom_data: HashMap::new(),
            computer_use_host: None,
            runtime_tool_restrictions: ToolRuntimeRestrictions::default(),
            runtime_handles: openbitfun_runtime_ports::ToolRuntimeHandles::default(),
        }
    }

    async fn start_tagged_capture(
        capture_id: &str,
        agent_session_id: Option<&str>,
        tty: bool,
    ) -> mpsc::UnboundedSender<ExecOutputChunk> {
        background_command_output_capture()
            .start_capture(StartBackgroundCommandOutputCapture {
                capture_id: capture_id.to_string(),
                agent_session_id: agent_session_id.map(str::to_string),
                command: "printf test".to_string(),
                workdir: None,
                remote: false,
                tty,
                terminal_size: None,
            })
            .await
    }

    #[tokio::test]
    async fn read_returns_snapshot_then_incremental_and_line_selection() {
        let capture_id = unique_capture_id("read-tool");
        let tx = start_tagged_capture(&capture_id, None, false).await;
        tx.send(ExecOutputChunk::combined("alpha\nbeta\n")).unwrap();
        tx.send(ExecOutputChunk::combined("gamma\n")).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let tool = ExecReadOutputTool::new();
        let snapshot = tool
            .call_read(
                &json!({
                    "action": "read",
                    "output_ref": capture_id,
                }),
                &tool_context(None),
            )
            .await
            .expect("snapshot read should succeed");
        let data = result_data(&snapshot);
        assert_eq!(data["status"], "ok");
        assert_eq!(data["output"], "alpha\nbeta\ngamma\n");
        let cursor = data["cursor"].as_u64().expect("cursor present");

        tx.send(ExecOutputChunk::combined("delta\n")).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let incremental = tool
            .call_read(
                &json!({
                    "action": "read",
                    "output_ref": capture_id,
                    "cursor": cursor,
                }),
                &tool_context(None),
            )
            .await
            .expect("incremental read should succeed");
        let data = result_data(&incremental);
        assert_eq!(data["output"], "delta\n");

        let selection = tool
            .call_read(
                &json!({
                    "action": "read",
                    "output_ref": capture_id,
                    "filter": "^(alpha|beta|delta)$",
                }),
                &tool_context(None),
            )
            .await
            .expect("line read should succeed");
        let data = result_data(&selection);
        assert_eq!(data["output"], "1| alpha\n2| beta\n4| delta");
        assert_eq!(data["matched_lines"], 3);
    }

    #[tokio::test]
    async fn read_answers_not_found_with_retention_policy() {
        let tool = ExecReadOutputTool::new();
        let results = tool
            .call_read(
                &json!({
                    "action": "read",
                    "output_ref": "never-existed",
                }),
                &tool_context(None),
            )
            .await
            .expect("read should answer structurally");
        let data = result_data(&results);

        assert_eq!(data["status"], "output_not_found");
        assert!(data["message"]
            .as_str()
            .expect("message")
            .contains("retained"));
    }

    #[tokio::test]
    async fn tty_stream_selection_degrades_loudly() {
        let capture_id = unique_capture_id("tty-read");
        let tx = start_tagged_capture(&capture_id, None, true).await;
        tx.send(ExecOutputChunk::combined("merged\n")).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let tool = ExecReadOutputTool::new();
        let results = tool
            .call_read(
                &json!({
                    "action": "read",
                    "output_ref": capture_id,
                    "stream": "stderr",
                }),
                &tool_context(None),
            )
            .await
            .expect("read should answer structurally");
        let data = result_data(&results);

        assert_eq!(data["status"], "unsupported");
        assert_eq!(data["reason"], "tty_session_combined_streams");
    }

    #[tokio::test]
    async fn stream_filter_keeps_only_requested_stream() {
        let capture_id = unique_capture_id("stream-read");
        let tx = start_tagged_capture(&capture_id, None, false).await;
        tx.send(ExecOutputChunk {
            stream: ExecOutputStream::Stdout,
            text: "out-line\n".to_string(),
        })
        .unwrap();
        tx.send(ExecOutputChunk {
            stream: ExecOutputStream::Stderr,
            text: "err-line\n".to_string(),
        })
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let tool = ExecReadOutputTool::new();
        let results = tool
            .call_read(
                &json!({
                    "action": "read",
                    "output_ref": capture_id,
                    "stream": "stderr",
                }),
                &tool_context(None),
            )
            .await
            .expect("stream read should succeed");
        let data = result_data(&results);

        assert_eq!(data["output"], "err-line\n");
    }

    #[tokio::test]
    async fn list_scopes_to_the_current_agent_session() {
        let mine = unique_capture_id("list-mine");
        let other = unique_capture_id("list-other");
        let tx = start_tagged_capture(&mine, Some("session-a"), false).await;
        tx.send(ExecOutputChunk::combined("x")).unwrap();
        let other_tx = start_tagged_capture(&other, Some("session-b"), false).await;
        other_tx.send(ExecOutputChunk::combined("y")).unwrap();
        background_command_output_capture()
            .update_lifecycle(&mine, 777, BackgroundCommandOutputStatus::Running, None)
            .await;
        background_command_output_capture()
            .update_lifecycle(&other, 778, BackgroundCommandOutputStatus::Running, None)
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let tool = ExecReadOutputTool::new();
        let results = tool
            .call_list(&tool_context(Some("session-a")))
            .await
            .expect("list should succeed");
        let data = result_data(&results);

        assert_eq!(data["count"], 1);
        assert_eq!(data["activities"][0]["output_ref"], mine);
        assert_eq!(data["activities"][0]["session_id"], 777);
    }

    #[tokio::test]
    async fn readonly_tool_declares_no_permission_intents() {
        let tool = ExecReadOutputTool::new();
        assert!(tool.is_readonly());
        let intents = tool
            .permission_intents(&json!({"action": "list"}), &tool_context(None))
            .expect("intents should build");
        assert!(intents.is_empty());
    }

    #[test]
    fn retention_policy_message_reports_the_real_bounds() {
        let message = background_command_output_retention_policy_message();
        assert!(message.contains(&format!(
            "{} KB",
            BACKGROUND_COMMAND_OUTPUT_CAPTURE_LIMIT_BYTES / 1024
        )));
        assert!(message.contains(&MAX_BACKGROUND_COMMAND_OUTPUT_RECORDS.to_string()));
    }
}
