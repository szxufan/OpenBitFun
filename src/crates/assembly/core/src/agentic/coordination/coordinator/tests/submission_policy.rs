use super::super::{
    btw_session_memory_mode, permission_mode_from_metadata, resolve_agent_submission_turn_id,
    resolve_submission_permission_mode, runtime_tool_restrictions_for_session_lifetime,
    SessionMemoryMode,
};
use crate::agentic::execution::restrict_recovered_permission_mode;
use openbitfun_runtime_ports::{AgentSubmissionRequest, AgentSubmissionSource, PermissionMode};

#[test]
fn submission_permission_mode_prefers_turn_then_session_then_global() {
    use openbitfun_runtime_ports::PermissionModeSource;

    let global_only = resolve_submission_permission_mode(None, None, PermissionMode::Ask);
    assert_eq!(global_only.mode, PermissionMode::Ask);
    assert_eq!(global_only.source, PermissionModeSource::GlobalDefault);

    // A session override isolates this session from the global default.
    let session_scoped = resolve_submission_permission_mode(
        None,
        Some(PermissionMode::FullAccess),
        PermissionMode::Ask,
    );
    assert_eq!(session_scoped.mode, PermissionMode::FullAccess);
    assert_eq!(session_scoped.source, PermissionModeSource::Session);

    // A one-off submission selection wins over the session's own mode,
    // including when it tightens the session back down.
    let turn_scoped = resolve_submission_permission_mode(
        Some(PermissionMode::Ask),
        Some(PermissionMode::FullAccess),
        PermissionMode::AutoApprove,
    );
    assert_eq!(turn_scoped.mode, PermissionMode::Ask);
    assert_eq!(turn_scoped.source, PermissionModeSource::Turn);
}

#[test]
fn recovered_permission_never_exceeds_original_or_current_mode() {
    assert_eq!(
        restrict_recovered_permission_mode(PermissionMode::FullAccess, PermissionMode::Ask),
        PermissionMode::Ask
    );
    assert_eq!(
        restrict_recovered_permission_mode(PermissionMode::Ask, PermissionMode::FullAccess),
        PermissionMode::Ask
    );
    assert_eq!(
        restrict_recovered_permission_mode(PermissionMode::FullAccess, PermissionMode::AutoApprove,),
        PermissionMode::AutoApprove
    );
}

#[test]
fn submission_metadata_mode_accepts_surface_aliases_and_rejects_unknown_values() {
    assert_eq!(
        permission_mode_from_metadata(Some(&serde_json::json!({
            "permission_mode": "auto",
        }))),
        Some(PermissionMode::AutoApprove)
    );
    assert_eq!(
        permission_mode_from_metadata(Some(&serde_json::json!({
            "permission_mode": "full_access",
        }))),
        Some(PermissionMode::FullAccess)
    );
    assert_eq!(
        permission_mode_from_metadata(Some(&serde_json::json!({
            "permission_mode": "elevated",
        }))),
        None
    );
    assert_eq!(permission_mode_from_metadata(None), None);
    assert_eq!(
        permission_mode_from_metadata(Some(&serde_json::json!({ "other": "ask" }))),
        None
    );
}

#[test]
fn btw_session_memory_mode_requires_both_generation_switches() {
    assert_eq!(
        btw_session_memory_mode(false, false),
        SessionMemoryMode::Disabled
    );
    assert_eq!(
        btw_session_memory_mode(false, true),
        SessionMemoryMode::Disabled
    );
    assert_eq!(
        btw_session_memory_mode(true, false),
        SessionMemoryMode::Disabled
    );
    assert_eq!(
        btw_session_memory_mode(true, true),
        SessionMemoryMode::Enabled
    );
}

#[test]
fn transient_session_runtime_restrictions_deny_out_of_band_session_tools() {
    let mut base = crate::agentic::tools::ToolRuntimeRestrictions::default();
    base.denied_tool_names.insert("ExecCommand".to_string());

    let transient = runtime_tool_restrictions_for_session_lifetime(base.clone(), true);
    for tool_name in [
        "SessionControl",
        "SessionMessage",
        "SessionHistory",
        "Cron",
        "ControlHub",
    ] {
        assert!(
            !transient.is_tool_allowed(tool_name),
            "{tool_name} must not cross a connection-scoped Session boundary"
        );
    }
    assert!(!transient.is_tool_allowed("ExecCommand"));
    assert!(transient.is_tool_allowed("Read"));

    let durable = runtime_tool_restrictions_for_session_lifetime(base, false);
    for tool_name in [
        "SessionControl",
        "SessionMessage",
        "SessionHistory",
        "Cron",
        "ControlHub",
    ] {
        assert!(durable.is_tool_allowed(tool_name));
    }
    assert!(!durable.is_tool_allowed("ExecCommand"));
}

#[test]
fn agent_submission_turn_id_prefers_explicit_field_over_metadata() {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "turnId".to_string(),
        serde_json::Value::String("legacy_metadata_turn".to_string()),
    );
    let request = AgentSubmissionRequest {
        session_id: "session_1".to_string(),
        message: "hello".to_string(),
        turn_id: Some("explicit_turn".to_string()),
        source: Some(AgentSubmissionSource::RemoteRelay),
        attachments: Vec::new(),
        metadata,
    };

    assert_eq!(resolve_agent_submission_turn_id(&request), "explicit_turn");
}

#[test]
fn agent_submission_turn_id_keeps_metadata_fallback() {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "turnId".to_string(),
        serde_json::Value::String("legacy_metadata_turn".to_string()),
    );
    let request = AgentSubmissionRequest {
        session_id: "session_1".to_string(),
        message: "hello".to_string(),
        turn_id: None,
        source: Some(AgentSubmissionSource::RemoteRelay),
        attachments: Vec::new(),
        metadata,
    };

    assert_eq!(
        resolve_agent_submission_turn_id(&request),
        "legacy_metadata_turn"
    );
}
