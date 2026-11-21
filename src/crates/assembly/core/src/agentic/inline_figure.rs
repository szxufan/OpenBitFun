//! The identity of a replayable FlowChat figure belongs to the producing session.
use std::path::Path;

use super::tools::framework::{ToolPathResolution, ToolUseContext};

const URI_PREFIX: &str = "openbitfun://current-session/artifacts/visualizations/v2/";
pub(super) const MAX_SOURCE_BYTES: usize = 1024 * 1024;

pub(super) fn figure_uri(path: &str, session_dir: Option<&Path>) -> Option<String> {
    let filename = if let Some(name) = path.strip_prefix(URI_PREFIX) {
        name
    } else {
        let path = Path::new(path);
        if !path.is_absolute() || path.parent()? != session_dir?.join("artifacts/visualizations/v2")
        {
            return None;
        }
        path.file_name()?.to_str()?
    };
    let slug = filename.strip_suffix(".html")?;
    if slug.is_empty()
        || slug.len() > 96
        || slug.starts_with('-')
        || !slug
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return None;
    }
    Some(format!("{URI_PREFIX}{filename}"))
}

pub(super) fn written_figure_uri(
    context: &ToolUseContext,
    path: &ToolPathResolution,
) -> Option<String> {
    if path.uses_remote_workspace_backend() {
        return None;
    }
    if path.logical_path.starts_with(URI_PREFIX) {
        return figure_uri(&path.logical_path, None);
    }
    if !path.logical_path.ends_with(".html") {
        return None;
    }
    let root = context
        .current_workspace_session_dir(context.session_id.as_deref()?)
        .ok()?;
    figure_uri(&path.resolved_path, Some(&root))
}

pub(super) fn reference_hint(uri: &str) -> String {
    format!("\nFlowChat attaches this figure to your final reply automatically. Reply in ordinary prose. For explicit placement among several figures, the optional standalone reference is:\nvisualize{}", serde_json::json!({ "path": uri }))
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FigureCheckRequest {
    pub source: String,
    pub user_request: Option<String>,
    pub uri: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub tool_call_id: Option<String>,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FigureCheckResult {
    pub rendered_source: Option<String>,
    pub bindings: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub deferred: bool,
}

/// Implemented by the producing host. Headless runtimes defer the check to the reader.
pub type FigureVerifier = std::sync::Arc<
    dyn Fn(
            FigureCheckRequest,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<FigureCheckResult, String>> + Send>,
        > + Send
        + Sync,
>;
static VERIFIER: std::sync::OnceLock<FigureVerifier> = std::sync::OnceLock::new();
pub fn register_figure_verifier(verifier: FigureVerifier) {
    let _ = VERIFIER.set(verifier);
}

pub(super) async fn verify_written_figure(
    context: &ToolUseContext,
    uri: &str,
    source: &str,
    data: &mut serde_json::Value,
    message: &mut String,
) {
    if context
        .custom_data
        .get(openbitfun_agent_runtime::remote_file_delivery::TOOL_CONTEXT_REMOTE_FILE_DELIVERY_KEY)
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        data["visualization_checked"] = false.into();
        message.push_str("\nThis reader needs a downloadable file. Write a standalone SVG or HTML in the workspace and include its computer:// link in the final answer.");
        return;
    }
    if source.trim().is_empty() || source.len() > MAX_SOURCE_BYTES {
        data["visualization_checked"] = false.into();
        message.push_str("\nThis revision cannot be published. Write a nonempty figure of at most 1 MiB before replying.");
        return;
    }
    let Some(verifier) = VERIFIER.get() else {
        data["visualization_check_deferred"] = true.into();
        message.push_str("\nThis runtime has no background renderer. FlowChat will check this revision on display.");
        message.push_str(&reference_hint(uri));
        return;
    };
    let request = FigureCheckRequest {
        source: source.to_owned(),
        user_request: context
            .custom_data
            .get(super::tools::tool_context_runtime::TURN_USER_REQUEST)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        uri: uri.into(),
        session_id: context.session_id.clone(),
        turn_id: context.dialog_turn_id.clone(),
        tool_call_id: context.tool_call_id.clone(),
    };
    match verifier(request).await.and_then(|rendered| {
        if rendered
            .rendered_source
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > MAX_SOURCE_BYTES)
        {
            Err("Invalid rendered figure size".to_owned())
        } else {
            Ok(rendered)
        }
    }) {
        Ok(rendered) => {
            if rendered.deferred {
                data["visualization_check_deferred"] = true.into();
                message.push_str("\nThe producing turn is unavailable to the background renderer. FlowChat will verify this revision with its recorded data on display.");
                message.push_str(&reference_hint(uri));
                return;
            }
            if let Some(bindings) = rendered.bindings {
                data["visualization_bindings"] = bindings.into();
            }
            if let Some(html) = rendered.rendered_source {
                data["visualization_rendered_bytes"] = html.len().into();
                data["visualization_rendered_source"] = html.into();
            }
            data["visualization_checked"] = true.into();
            message
                .push_str("\nFlowChat rendered and checked this exact revision in the background.");
            message.push_str(&reference_hint(uri));
        }
        Err(issue) => {
            data["visualization_checked"] = false.into();
            let issue: String = issue.chars().take(500).collect();
            message.push_str(&format!("\nFlowChat could not publish this revision: {issue}. Fix the fragment using Edit or Write at the same destination. The host repeats the check automatically."));
        }
    }
}
