//! Tool framework - Tool interface definition and execution context
pub use crate::agentic::tools::tool_context_runtime::ToolUseContext;
use crate::util::errors::OpenBitFunResult;
use async_trait::async_trait;
pub use openbitfun_agent_tools::{
    build_tool_path_policy_denial_message, build_tool_runtime_artifact_reference,
    build_tool_session_runtime_artifact_reference, is_tool_path_allowed_by_resolved_roots,
    resolve_tool_path_with_context, resolve_tool_path_with_context_roots,
    tool_path_is_effectively_absolute, DynamicMcpToolInfo, DynamicToolInfo, PermissionIntent,
    PortableToolContextProvider, ToolContextFacts, ToolExposure, ToolPathBackend,
    ToolPathResolution, ToolRenderOptions, ToolResult, ToolWorkspaceKind, ValidationResult,
};
use serde_json::Value;

/// Tool trait
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name
    fn name(&self) -> &str;

    /// Tool description
    async fn description(&self) -> OpenBitFunResult<String>;

    /// Tool description with execution context.
    async fn description_with_context(
        &self,
        _context: Option<&ToolUseContext>,
    ) -> OpenBitFunResult<String> {
        self.description().await
    }

    /// Short description used in condensed tool listings such as GetToolSpec.
    fn short_description(&self) -> String;

    /// Default exposure level when building the model tool manifest.
    ///
    /// This is tool-owned metadata: registries and agent manifests may use it
    /// as the baseline before applying any higher-level overrides.
    fn default_exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    /// Input mode definition - using JSON Schema
    fn input_schema(&self) -> Value;

    /// JSON Schema for the assistant-facing portion of the value returned to a
    /// PTC program.
    ///
    /// This is intentionally a compact semantic projection, rather than a
    /// mirror of every field in the internal/canonical result. Include fields
    /// that are present in `result_for_assistant` or that the model needs for
    /// control flow (for example a continuation cursor or an operation id).
    /// Omit UI-only, diagnostic, persistence, and transport metadata. The
    /// canonical result may still contain those fields at runtime; they are
    /// simply not part of the model-facing SDK contract. The default is an
    /// open/unknown JSON value so existing tools remain compatible while
    /// output contracts are added incrementally.
    fn output_schema(&self) -> Value {
        Value::Object(serde_json::Map::new())
    }

    /// JSON Schema sent to the model (may depend on app language or other runtime config).
    /// Default: same as [`input_schema`].
    async fn input_schema_for_model(&self) -> Value {
        self.input_schema()
    }

    /// JSON Schema for the model when tool listing has a [`ToolUseContext`] (e.g. primary model vision capability).
    /// Default: ignores context and delegates to [`input_schema_for_model`].
    async fn input_schema_for_model_with_context(&self, context: Option<&ToolUseContext>) -> Value {
        let _ = context;
        self.input_schema_for_model().await
    }

    /// Input JSON Schema - optional extra schema
    fn input_json_schema(&self) -> Option<Value> {
        None
    }

    /// MCP Apps: URI of UI resource (ui://) declared in tool metadata. Used when tool result
    /// does not contain a resource - the host fetches from this pre-declared URI.
    fn ui_resource_uri(&self) -> Option<String> {
        None
    }

    /// Dynamic tool provider identity used by boundary adapters.
    ///
    /// Keep this as explicit metadata instead of deriving ownership from tool
    /// names so future tool registries can change naming without breaking
    /// provider routing.
    fn dynamic_provider_id(&self) -> Option<&str> {
        None
    }

    /// Stable contextual dispatch target for providers that multiplex same-name tools.
    fn deferred_spec_route_id(&self, _context: &ToolUseContext) -> Option<String> {
        None
    }

    /// Rich metadata for dynamic tools. Prefer this over encoding dynamic ownership in tool names.
    fn dynamic_tool_info(&self) -> Option<DynamicToolInfo> {
        self.dynamic_provider_id()
            .map(|provider_id| DynamicToolInfo {
                provider_id: provider_id.to_string(),
                provider_kind: None,
                mcp: None,
            })
    }

    /// User friendly name
    fn user_facing_name(&self) -> String {
        self.name().to_string()
    }

    /// Whether to enable
    async fn is_enabled(&self) -> bool {
        true
    }

    /// Whether this tool is available for a specific execution context.
    async fn is_available_in_context(&self, _context: Option<&ToolUseContext>) -> bool {
        self.is_enabled().await
    }

    /// Whether to be readonly
    fn is_readonly(&self) -> bool {
        false
    }

    /// Whether to be concurrency safe
    fn is_concurrency_safe(&self, _input: Option<&Value>) -> bool {
        self.is_readonly()
    }

    /// Subsequent calls must be preflighted after this tool finishes because
    /// it can change session execution policy (for example, activate hooks).
    fn invalidates_tool_preflight(&self) -> bool {
        false
    }

    /// Describe permission actions and resources without performing side effects.
    fn permission_intents(
        &self,
        _input: &Value,
        _context: &ToolUseContext,
    ) -> OpenBitFunResult<Vec<PermissionIntent>> {
        if self.is_readonly() {
            Ok(Vec::new())
        } else {
            Ok(vec![PermissionIntent::new(
                "custom_tool",
                vec![self.name().to_string()],
            )])
        }
    }

    /// Host-aware, non-executing preparation after input validation and Hook rewrites.
    async fn prepare_permission_intents(
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> OpenBitFunResult<Vec<PermissionIntent>> {
        self.permission_intents(input, context)
    }

    /// Release invocation-owned host selections after completion or cancellation.
    async fn release_permission_preparation(&self, _input: &Value, _context: &ToolUseContext) {}

    /// Whether this tool manages its own execution timeout (for example via the
    /// subagent timeout handle) and should not be wrapped by the global tool
    /// pipeline timeout.
    fn manages_own_execution_timeout(&self) -> bool {
        false
    }

    /// Whether a pending round injection may end this tool normally so the
    /// current turn can advance without cancelling the underlying work.
    fn round_injection_yieldable(&self) -> bool {
        false
    }

    /// Whether to support streaming output
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Validate input
    async fn validate_input(
        &self,
        _input: &Value,
        _context: Option<&ToolUseContext>,
    ) -> ValidationResult {
        ValidationResult {
            result: true,
            message: None,
            error_code: None,
            meta: None,
        }
    }

    /// Validate invariants on the original input that a Hook rewrite must not
    /// relax. The default preserves existing tool behavior while discarding
    /// ordinary repairable validation failures.
    ///
    /// Tools whose full [`validate_input`] can return a repairable error before
    /// reaching an invariant must override this method and inspect every
    /// independently recognizable protected field in the partial input.
    async fn validate_input_rewrite_invariants(
        &self,
        input: &Value,
        context: Option<&ToolUseContext>,
    ) -> ValidationResult {
        let validation = self.validate_input(input, context).await;
        if validation.blocks_input_rewrite() {
            validation
        } else {
            ValidationResult::default()
        }
    }

    /// Render result for assistant
    fn render_result_for_assistant(&self, _output: &Value) -> String {
        "Tool result".to_string()
    }

    /// Render tool use message
    fn render_tool_use_message(&self, input: &Value, _options: &ToolRenderOptions) -> String {
        format!("Using {}: {}", self.name(), input)
    }

    /// Render tool use rejected message
    fn render_tool_use_rejected_message(&self) -> String {
        format!("{} tool use was rejected", self.name())
    }

    /// Render tool result message
    fn render_tool_result_message(&self, _output: &Value) -> String {
        format!("{} completed", self.name())
    }

    /// Execute the tool's concrete business logic.
    /// Implementors should put the actual tool behavior here and assume
    /// [`call`] will wrap it with cross-cutting concerns such as cancellation.
    async fn call_impl(
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> OpenBitFunResult<Vec<ToolResult>>;

    /// Unified tool entry point.
    /// This method owns shared framework behavior and delegates the actual
    /// execution to [`call_impl`], so most tools should override `call_impl`
    /// instead of overriding this method directly.
    async fn call(
        &self,
        input: &Value,
        context: &ToolUseContext,
    ) -> OpenBitFunResult<Vec<ToolResult>> {
        crate::agentic::tools::tool_context_runtime::call_tool_with_runtime_hooks(
            self, input, context,
        )
        .await
    }
}

#[cfg(test)]
mod permission_defaults_tests {
    use super::*;
    use crate::agentic::tools::implementations::*;
    use crate::agentic::WorkspaceBinding;
    use openbitfun_agent_runtime::permission::{plan_permission_intents, PermissionIntentPlan};
    use openbitfun_runtime_ports::{
        PermissionEffect, PermissionPolicyPreset, PermissionResourceCaseSensitivity,
        PermissionRule, ResolvedPermissionPolicy,
    };
    use serde_json::json;

    fn plan(
        tool: &dyn Tool,
        input: Value,
        context: &ToolUseContext,
        extra: Vec<PermissionRule>,
    ) -> PermissionIntentPlan {
        let mut rules = PermissionPolicyPreset::Ask.baseline_rules();
        rules.extend(extra);
        plan_permission_intents(
            tool.permission_intents(&input, context)
                .expect("permission intents"),
            &ResolvedPermissionPolicy::new(rules, Vec::new()),
            &[],
            PermissionResourceCaseSensitivity::Sensitive,
        )
    }

    #[test]
    fn permission_defaults_allow_requested_tools_and_preserve_explicit_rules() {
        let context = ToolUseContext::for_tool_listing(None, None);
        let tools: Vec<(Box<dyn Tool>, Value)> = vec![
            (
                Box::new(ExecControlTool::new()),
                json!({"session_id":1,"action":"kill"}),
            ),
            (Box::new(CronTool::new()), json!({"action":"create"})),
            (
                Box::new(SessionMessageTool::new()),
                json!({"session_id":"other","message":"hello"}),
            ),
            (Box::new(GetGoalTool::new()), json!({})),
            (Box::new(CreateGoalTool::new()), json!({"objective":"work"})),
            (
                Box::new(UpdateGoalTool::new()),
                json!({"status":"complete"}),
            ),
            #[cfg(feature = "tools-miniapp")]
            (Box::new(InitMiniAppTool::new()), json!({"name":"demo"})),
            #[cfg(feature = "tools-miniapp")]
            (
                Box::new(FinalizeMiniAppTool::new()),
                json!({"app_id":"demo"}),
            ),
            #[cfg(feature = "tools-image-generation")]
            (
                Box::new(GenerateImageTool::new()),
                json!({"service_id":"images"}),
            ),
        ];
        for (tool, input) in tools {
            assert_eq!(
                plan(tool.as_ref(), input.clone(), &context, vec![]),
                PermissionIntentPlan::Allowed,
                "{}",
                tool.name()
            );
            let resources = tool.permission_intents(&input, &context).unwrap();
            let resource = resources[0].resources[0].clone();
            let expected_action = match tool.name() {
                "ExecControl" => "process.control",
                "Cron" => "schedule.manage",
                "SessionMessage" => "session.message",
                "get_goal" => "goal.read",
                "create_goal" => "goal.create",
                "update_goal" => "goal.update",
                "InitMiniApp" => "miniapp.create",
                "FinalizeMiniApp" => "miniapp.finalize",
                "generate_image" => "image.generate",
                name => panic!("Missing action expectation for {name}"),
            };
            assert_eq!(resources[0].action, expected_action);
            for effect in [PermissionEffect::Ask, PermissionEffect::Deny] {
                let result = plan(
                    tool.as_ref(),
                    input.clone(),
                    &context,
                    vec![PermissionRule::new(expected_action, &resource, effect)],
                );
                match effect {
                    PermissionEffect::Ask => assert!(
                        matches!(result, PermissionIntentPlan::RequiresApproval(_)),
                        "{}",
                        tool.name()
                    ),
                    PermissionEffect::Deny => assert!(
                        matches!(result, PermissionIntentPlan::Denied(_)),
                        "{}",
                        tool.name()
                    ),
                    _ => unreachable!(),
                }
            }
        }
    }

    #[test]
    fn permission_defaults_split_queries_from_mutations_with_domain_rules() {
        let context = ToolUseContext::for_tool_listing(None, None);
        let cases: Vec<(Box<dyn Tool>, Value, Value)> = vec![
            (
                Box::new(WriteStdinTool::new()),
                json!({"session_id":1}),
                json!({"session_id":1,"chars":"yes","append_enter":true}),
            ),
            (
                Box::new(WriteStdinTool::new()),
                json!({"session_id":1,"chars":""}),
                json!({"session_id":1,"chars":"","append_enter":true}),
            ),
            (
                Box::new(SessionControlTool::new()),
                json!({"action":"list"}),
                json!({"action":"delete","session_id":"other"}),
            ),
            #[cfg(feature = "tools-browser-web")]
            (
                Box::new(ControlHubTool::new()),
                json!({"domain":"browser","action":"snapshot"}),
                json!({"domain":"browser","action":"evaluate","params":{"expression":"submit()"}}),
            ),
            #[cfg(feature = "tools-browser-web")]
            (
                Box::new(ControlHubTool::new()),
                json!({"domain":"browser","action":"read_article"}),
                json!({"domain":"browser","action":"read_article","params":{"url":"https://example.com"}}),
            ),
            #[cfg(feature = "tools-browser-web")]
            (
                Box::new(ControlHubTool::new()),
                json!({"domain":"browser","action":"get","params":{"selector":"@e1"}}),
                json!({"domain":"browser","action":"click","params":{"selector":"@e1"}}),
            ),
            #[cfg(feature = "tools-browser-web")]
            (
                Box::new(ControlHubTool::new()),
                json!({"domain":"terminal","action":"list_sessions"}),
                json!({"domain":"terminal","action":"kill"}),
            ),
            #[cfg(feature = "tools-git")]
            (
                Box::new(ReviewPlatformTool::new()),
                json!({"action":"get_pull_request"}),
                json!({"action":"approve_pull_request"}),
            ),
            #[cfg(feature = "tools-miniapp")]
            (
                Box::new(MiniAppBrowserTool::new()),
                json!({"action":"inspect"}),
                json!({"action":"act_bridge"}),
            ),
        ];
        for (tool, read, write) in cases {
            assert_eq!(
                plan(tool.as_ref(), read.clone(), &context, vec![]),
                PermissionIntentPlan::Allowed,
                "{}",
                tool.name()
            );
            assert!(
                matches!(
                    plan(tool.as_ref(), write, &context, vec![]),
                    PermissionIntentPlan::RequiresApproval(_)
                ),
                "{}",
                tool.name()
            );
            let intents = tool.permission_intents(&read, &context).unwrap();
            let read_action = &intents[0].action;
            assert_ne!(read_action, "custom_tool");
            assert!(
                matches!(
                    plan(
                        tool.as_ref(),
                        read.clone(),
                        &context,
                        vec![PermissionRule::new(
                            read_action,
                            "*",
                            PermissionEffect::Deny
                        )]
                    ),
                    PermissionIntentPlan::Denied(_)
                ),
                "{}",
                tool.name()
            );
            // Old name-based custom-tool rules are no longer aliases.
            assert_eq!(
                plan(
                    tool.as_ref(),
                    read,
                    &context,
                    vec![PermissionRule::new(
                        "custom_tool",
                        tool.name(),
                        PermissionEffect::Deny
                    )]
                ),
                PermissionIntentPlan::Allowed
            );
        }
    }

    #[test]
    fn permission_defaults_use_concrete_domain_resources() {
        let context = ToolUseContext::for_tool_listing(None, None);
        let input = WriteStdinTool::new()
            .permission_intents(&json!({"session_id":17,"chars":"yes"}), &context)
            .unwrap();
        assert_eq!(input[0].action, "process.write_input");
        assert_eq!(input[0].resources, ["exec:17"]);
        let control = ExecControlTool::new()
            .permission_intents(&json!({"session_id":18,"action":"kill"}), &context)
            .unwrap();
        assert_eq!(control[0].action, "process.control");
        assert_eq!(control[0].resources, ["exec:18"]);
        let message = SessionMessageTool::new().permission_intents(&json!({"workspace":"/target/workspace", "session_id":"recipient", "message":"private body"}), &context).unwrap();
        assert_eq!(message[0].action, "session.message");
        assert_eq!(
            message[0].resources,
            ["/target/workspace:session:recipient"]
        );
        assert!(!format!("{:?}", message[0]).contains("private body"));
        #[cfg(feature = "tools-browser-web")]
        {
            let tool = ControlHubTool::new();
            let browser = tool.permission_intents(&json!({"domain":"browser","action":"click","params":{"target":"builtin","session_id":"page-1"}}), &context).unwrap();
            assert_eq!(browser[0].action, "browser.interact");
            assert_eq!(browser[0].resources, ["builtin:page-1"]);
            assert_eq!(browser[0].save_resources, browser[0].resources);
            let implicit = tool
                .permission_intents(&json!({"domain":"browser","action":"click"}), &context)
                .unwrap();
            assert!(implicit[0].save_resources.is_empty());
            let terminal = tool.permission_intents(&json!({"domain":"terminal","action":"kill","params":{"terminal_session_id":"user-shell"}}), &context).unwrap();
            assert_eq!(terminal[0].action, "terminal.control");
            assert_eq!(terminal[0].resources, ["terminal:user-shell"]);
            assert!(matches!(
                plan(
                    &tool,
                    json!({"domain":"terminal","action":"kill"}),
                    &context,
                    vec![PermissionRule::new(
                        "process.control",
                        "*",
                        PermissionEffect::Allow
                    )]
                ),
                PermissionIntentPlan::RequiresApproval(_)
            ));
            assert!(tool
                .permission_intents(&json!({"domain":"unknown", "action":"read"}), &context)
                .is_err());
        }
        #[cfg(feature = "tools-miniapp")]
        {
            let browser = MiniAppBrowserTool::new()
                .permission_intents(
                    &json!({"action":"inspect","sessionId":"miniapp-session"}),
                    &context,
                )
                .unwrap();
            assert_eq!(browser[0].action, "miniapp.read");
            assert_eq!(browser[0].resources, ["session:miniapp-session"]);
            let publish = PublishMiniAppTool::new();
            assert_eq!(
                publish
                    .permission_intents(&json!({"app_id":"demo"}), &context)
                    .unwrap()[0]
                    .action,
                "miniapp.publish"
            );
            assert!(matches!(
                plan(&publish, json!({"app_id":"demo"}), &context, vec![]),
                PermissionIntentPlan::RequiresApproval(_)
            ));
        }
    }

    #[test]
    fn permission_defaults_file_edits_preserve_sensitive_delete_and_external_gates() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let context =
            ToolUseContext::for_tool_listing(Some(WorkspaceBinding::new(None, workspace)), None);
        let edit = FileEditTool::new();
        let write = FileWriteTool::new();
        assert_eq!(
            plan(&edit, json!({"file_path":"main.rs"}), &context, vec![]),
            PermissionIntentPlan::Allowed
        );
        assert_eq!(
            plan(
                &write,
                json!({"payload":"+++ main.rs\nhello"}),
                &context,
                vec![]
            ),
            PermissionIntentPlan::Allowed
        );
        for path in [
            ".env",
            ".env.local",
            "private.key",
            ".git/config",
            ".ssh/config",
        ] {
            assert!(
                matches!(
                    plan(&edit, json!({"file_path":path}), &context, vec![]),
                    PermissionIntentPlan::RequiresApproval(_)
                ),
                "{path}"
            );
            assert!(
                matches!(
                    plan(
                        &write,
                        json!({"payload":format!("+++ {path}\nhello")}),
                        &context,
                        vec![]
                    ),
                    PermissionIntentPlan::RequiresApproval(_)
                ),
                "{path}"
            );
        }
        assert_eq!(
            plan(&edit, json!({"file_path":".env.example"}), &context, vec![]),
            PermissionIntentPlan::Allowed
        );
        assert!(matches!(
            plan(
                &DeleteFileTool::new(),
                json!({"path":"main.rs"}),
                &context,
                vec![]
            ),
            PermissionIntentPlan::RequiresApproval(_)
        ));
        assert!(matches!(
            plan(
                &edit,
                json!({"file_path":root.path().join("outside.rs")}),
                &context,
                vec![]
            ),
            PermissionIntentPlan::RequiresApproval(_)
        ));
        assert!(matches!(
            plan(
                &edit,
                json!({"file_path":"main.rs"}),
                &context,
                vec![PermissionRule::new("edit", "*", PermissionEffect::Deny)]
            ),
            PermissionIntentPlan::Denied(_)
        ));
        assert!(matches!(
            plan(
                &DeleteFileTool::new(),
                json!({"path":"main.rs"}),
                &context,
                vec![PermissionRule::new("edit", "*", PermissionEffect::Deny)]
            ),
            PermissionIntentPlan::Denied(_)
        ));
        assert!(matches!(
            plan(
                &ExecCommandTool::new(),
                json!({"cmd":"git status"}),
                &context,
                vec![]
            ),
            PermissionIntentPlan::RequiresApproval(_)
        ));
    }

    #[cfg(feature = "tools-computer-use")]
    #[test]
    fn permission_defaults_computer_observation_keeps_input_and_scripts_guarded() {
        let context = ToolUseContext::for_tool_listing(None, None);
        let tool = ComputerUseTool::new();
        for action in [
            "screenshot",
            "get_app_state",
            "list_windows",
            "control_status",
            "wait",
        ] {
            assert_eq!(
                plan(&tool, json!({"action":action}), &context, vec![]),
                PermissionIntentPlan::Allowed,
                "{action}"
            );
        }
        for action in [
            "start_control",
            "click",
            "type_text",
            "clipboard_get",
            "clipboard_set",
            "run_script",
            "app_batch",
        ] {
            assert!(
                matches!(
                    plan(&tool, json!({"action":action}), &context, vec![]),
                    PermissionIntentPlan::RequiresApproval(_)
                ),
                "{action}"
            );
        }
    }
}
