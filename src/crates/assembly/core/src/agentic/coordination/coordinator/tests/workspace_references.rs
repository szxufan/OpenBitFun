use super::super::{ConversationCoordinator, SessionReferenceLocator};

#[test]
fn session_reference_artifact_stems_extend_only_for_collisions() {
    let references = vec![
        SessionReferenceLocator {
            session_id: "12345678aaaa0000".to_string(),
            workspace_id: None,
            workspace_path: "/workspace-a".to_string(),
            remote_connection_id: None,
            remote_ssh_host: None,
        },
        SessionReferenceLocator {
            session_id: "12345678bbbb0000".to_string(),
            workspace_id: None,
            workspace_path: "/workspace-b".to_string(),
            remote_connection_id: None,
            remote_ssh_host: None,
        },
        SessionReferenceLocator {
            session_id: "12345678aaaa0000".to_string(),
            workspace_id: None,
            workspace_path: "/workspace-a".to_string(),
            remote_connection_id: None,
            remote_ssh_host: None,
        },
    ];

    assert_eq!(
        ConversationCoordinator::session_reference_artifact_stems(&references),
        vec![
            "12345678".to_string(),
            "12345678bbbb".to_string(),
            "12345678".to_string(),
        ]
    );
}

#[test]
fn session_reference_display_name_normalizes_escapes_and_truncates() {
    assert_eq!(
        ConversationCoordinator::session_reference_display_name("  Fix\n auth | invalid \\ path  ",),
        "Fix auth \\| invalid \\\\ path"
    );
    assert_eq!(
        ConversationCoordinator::session_reference_display_name("\t\n"),
        "(untitled session)"
    );

    let long_name = "a".repeat(super::super::SESSION_REFERENCE_NAME_CHAR_LIMIT + 1);
    let display_name = ConversationCoordinator::session_reference_display_name(&long_name);
    assert_eq!(
        display_name.chars().count(),
        super::super::SESSION_REFERENCE_NAME_CHAR_LIMIT + 3
    );
    assert!(display_name.ends_with("..."));
}

#[test]
fn workspace_reference_source_validation_uses_unicode_character_offsets() {
    let reference = openbitfun_runtime_ports::AgentWorkspaceReference {
        path: "src/你.rs".to_string(),
        kind: openbitfun_runtime_ports::AgentWorkspaceReferenceKind::File,
        start_line: Some(2),
        end_line: Some(8),
        source: openbitfun_runtime_ports::AgentWorkspaceReferenceSourceRange {
            start: 3,
            end: 16,
            value: "@src/你.rs#2-8".to_string(),
        },
    };
    ConversationCoordinator::validate_workspace_reference_source("看看 @src/你.rs#2-8", &reference)
        .expect("valid character offsets should be accepted");
}

#[test]
fn workspace_reference_source_validation_rejects_stale_text_and_invalid_ranges() {
    let mut reference = openbitfun_runtime_ports::AgentWorkspaceReference {
        path: "src/lib.rs".to_string(),
        kind: openbitfun_runtime_ports::AgentWorkspaceReferenceKind::File,
        start_line: None,
        end_line: None,
        source: openbitfun_runtime_ports::AgentWorkspaceReferenceSourceRange {
            start: 4,
            end: 15,
            value: "@src/lib.rs".to_string(),
        },
    };
    assert!(
        ConversationCoordinator::validate_workspace_reference_source(
            "see @src/main.rs",
            &reference,
        )
        .is_err()
    );
    reference.source.end = 100;
    assert!(
            ConversationCoordinator::validate_workspace_reference_source(
                "see @src/lib.rs",
                &reference,
            )
            .is_err()
        );

    reference.source.end = 15;
    assert!(
        ConversationCoordinator::validate_workspace_reference_source(
            "see @src/lib.rsx",
            &reference,
        )
        .is_err()
    );
    reference.source.start = 1;
    reference.source.end = 12;
    assert!(
        ConversationCoordinator::validate_workspace_reference_source("x@src/lib.rs", &reference,)
            .is_err()
    );
}
