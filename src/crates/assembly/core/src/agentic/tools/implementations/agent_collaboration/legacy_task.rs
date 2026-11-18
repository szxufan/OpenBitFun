use super::*;

/// Legacy combined tool retained for existing custom-agent definitions.
pub struct TaskTool;

impl Default for TaskTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskTool {
    pub fn new() -> Self {
        Self
    }

    fn render_legacy_description(allow_model_override: bool) -> String {
        AgentExecutionTool::new()
            .render_description(allow_model_override)
            .replace("AgentSpawn", "Task")
            .replace("AgentSendInput", "Task")
            .replace("AgentControl", "Task")
    }
}

/// Compatibility implementation for custom agents that still explicitly list
/// the original combined collaboration tool. Built-in modes use the split
/// Agent* tools, while this entry point keeps existing custom-agent
/// definitions executable without a configuration migration.
#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "Task"
    }

    fn manages_own_execution_timeout(&self) -> bool {
        true
    }

    async fn description(&self) -> OpenBitFunResult<String> {
        Ok(Self::render_legacy_description(
            ModelOverridePolicy::presentation().await?.allowed,
        ))
    }

    async fn is_available_in_context(&self, _context: Option<&ToolUseContext>) -> bool {
        true
    }

    fn short_description(&self) -> String {
        "Delegate work to a subagent task and collect the result.".to_string()
    }

    async fn description_with_context(
        &self,
        _context: Option<&ToolUseContext>,
    ) -> OpenBitFunResult<String> {
        Ok(Self::render_legacy_description(
            ModelOverridePolicy::presentation().await?.allowed,
        ))
    }

    fn input_schema(&self) -> Value {
        AgentExecutionTool::regular_input_schema()
    }

    async fn input_schema_for_model(&self) -> Value {
        ModelOverridePolicy::model_schema(self.input_schema()).await
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": {"type": "string"},
                "agent_id": {"type": "string"},
                "bg_task_id": {"type": "string"},
                "result": {"type": "string"},
                "error": {"type": "string"}
            },
            "additionalProperties": true
        })
    }

    async fn input_schema_for_model_with_context(
        &self,
        _context: Option<&ToolUseContext>,
    ) -> Value {
        self.input_schema_for_model().await
    }

    fn is_readonly(&self) -> bool {
        false
    }

    fn is_concurrency_safe(&self, input: Option<&Value>) -> bool {
        AgentExecutionTool::new().is_concurrency_safe(input)
    }

    fn permission_intents(
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> OpenBitFunResult<Vec<PermissionIntent>> {
        AgentExecutionTool::new().permission_intents(input, context)
    }

    async fn validate_input(
        &self,
        input: &Value,
        context: Option<&ToolUseContext>,
    ) -> ValidationResult {
        if let Some(validation) = model_override_validation(input).await {
            return validation;
        }
        AgentExecutionTool::validate_invocation_input(
            input,
            false,
            context.and_then(ToolUseContext::workspace_root),
        )
        .await
    }

    fn render_tool_use_message(&self, input: &Value, options: &ToolRenderOptions) -> String {
        match TaskAction::parse(input).ok() {
            Some(TaskAction::Cancel) => input
                .get("agent_id")
                .and_then(Value::as_str)
                .map(|agent_id| format!("Cancelling background task: {agent_id}"))
                .unwrap_or_else(|| "Cancelling background task".to_string()),
            Some(TaskAction::SendInput) => input
                .get("agent_id")
                .and_then(Value::as_str)
                .map(|agent_id| {
                    if options.verbose {
                        format!("Sending input to task: {agent_id}")
                    } else {
                        format!("Task input: {agent_id}")
                    }
                })
                .unwrap_or_else(|| "Sending input to task".to_string()),
            Some(TaskAction::Spawn) | None => input
                .get("agent_id")
                .and_then(Value::as_str)
                .map(|agent_id| {
                    if options.verbose {
                        format!("Creating task: {agent_id}")
                    } else {
                        format!("Task: {agent_id}")
                    }
                })
                .unwrap_or_else(|| "Creating task".to_string()),
        }
    }

    async fn call_impl(
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> OpenBitFunResult<Vec<ToolResult>> {
        ModelOverridePolicy::enforce(input).await?;
        AgentExecutionTool::new()
            .call_task_impl(input, context)
            .await
    }
}
