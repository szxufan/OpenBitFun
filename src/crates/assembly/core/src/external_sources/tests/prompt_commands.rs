use super::super::*;
use openbitfun_product_domains::external_sources::{
    EcosystemId, ExternalSourceRecord, ExternalSourceScope, PromptCommandAvailability,
    PromptCommandCatalogEntry, PromptCommandConflict, PromptCommandConflictCandidate,
    PromptCommandDefinition, PromptCommandExecutionTarget, SourceQualifiedCommandId,
};

#[test]
fn native_prompt_command_choice_is_versioned_by_every_participant() {
    let source_key = SourceKey::new("opencode.commands", "project").unwrap();
    let definition = PromptCommandDefinition {
        id: SourceQualifiedCommandId::new(source_key.clone(), "review").unwrap(),
        name: "review".to_string(),
        description: "Review changes".to_string(),
        template: "Review changes".to_string(),
        shell_preference: None,
        execution_target: Default::default(),
        availability: PromptCommandAvailability::Available,
        content_version: "external-v1".to_string(),
    };
    let snapshot = ExternalSourceCatalogSnapshot {
        generation: 1,
        discovery_pending: false,
        sources: vec![ExternalSourceCatalogEntry {
            stable_key: source_key.stable_key(),
            presentation_group_id: None,
            record: ExternalSourceRecord {
                key: source_key,
                ecosystem_id: EcosystemId::new("opencode").unwrap(),
                display_name: "OpenCode project commands".to_string(),
                source_kind: "prompt_commands".to_string(),
                scope: ExternalSourceScope::Project,
                location: "/repo/.opencode/commands".to_string(),
                execution_domain_id: ExecutionDomainId::new("local-user").unwrap(),
                health:
                    openbitfun_product_domains::external_sources::ExternalSourceHealth::Available,
                content_version: "source-v1".to_string(),
                diagnostics: Vec::new(),
            },
            lifecycle: ExternalSourceLifecycleState::Available,
        }],
        commands: vec![PromptCommandCatalogEntry { definition }],
        command_conflicts: Vec::new(),
        tools: Vec::new(),
        tool_approval_requests: Vec::new(),
        tool_conflicts: Vec::new(),
        mcp_generation: 0,
        mcp_servers: Vec::new(),
        mcp_approval_requests: Vec::new(),
        mcp_conflicts: Vec::new(),
        subagent_generation: 0,
        preference_revision: 4,
        subagents: Vec::new(),
        subagent_conflicts: Vec::new(),
        pending_subagent_approvals: Vec::new(),
        subagent_model_binding_groups: Vec::new(),
        subagent_model_binding_options: Vec::new(),
        integration_policy: Default::default(),
        diagnostics: Vec::new(),
    };
    let native_v1 = NativePromptCommandDescriptor {
        command_name: "review".to_string(),
        candidate_id: "openbitfun.desktop:action:review".to_string(),
        behavior_version: "native-v1".to_string(),
    };
    let first = project_native_prompt_command_conflicts(
        &snapshot,
        std::slice::from_ref(&native_v1),
        &BTreeMap::new(),
        &BTreeSet::new(),
        4,
    )
    .unwrap();
    let first_conflict = first.conflicts.first().unwrap();
    assert_eq!(first_conflict.selected_candidate_id, None);

    let choices = BTreeMap::from([(
        first_conflict.conflict_key.clone(),
        native_v1.candidate_id.clone(),
    )]);
    let selected = project_native_prompt_command_conflicts(
        &snapshot,
        std::slice::from_ref(&native_v1),
        &choices,
        &BTreeSet::new(),
        5,
    )
    .unwrap();
    assert_eq!(
        selected.conflicts[0].selected_candidate_id.as_deref(),
        Some(native_v1.candidate_id.as_str())
    );

    let cli_surface = NativePromptCommandDescriptor {
        command_name: "review".to_string(),
        candidate_id: "openbitfun.cli:action:review".to_string(),
        behavior_version: "native-v1".to_string(),
    };
    let isolated = project_native_prompt_command_conflicts(
        &snapshot,
        &[cli_surface],
        &choices,
        &BTreeSet::new(),
        5,
    )
    .unwrap();
    assert_ne!(
        isolated.conflicts[0].conflict_key,
        first_conflict.conflict_key
    );
    assert_eq!(isolated.conflicts[0].selected_candidate_id, None);

    let removed_snapshot = ExternalSourceCatalogSnapshot {
        commands: Vec::new(),
        command_conflicts: Vec::new(),
        ..snapshot.clone()
    };
    let removed = project_native_prompt_command_conflicts(
        &removed_snapshot,
        std::slice::from_ref(&native_v1),
        &choices,
        &BTreeSet::from([native_v1.candidate_id.clone()]),
        5,
    )
    .unwrap();
    assert!(removed.conflicts.is_empty());
    assert_eq!(
        removed.reconfirmations,
        [NativePromptCommandReconfirmationProjection {
            command_name: "review".to_string(),
            native_candidate_id: native_v1.candidate_id.clone(),
        }]
    );

    let native_v2 = NativePromptCommandDescriptor {
        behavior_version: "native-v2".to_string(),
        ..native_v1
    };
    let changed = project_native_prompt_command_conflicts(
        &snapshot,
        &[native_v2],
        &choices,
        &BTreeSet::new(),
        5,
    )
    .unwrap();
    assert_ne!(
        changed.conflicts[0].conflict_key,
        first_conflict.conflict_key
    );
    assert_eq!(changed.conflicts[0].selected_candidate_id, None);
}

#[test]
fn delegated_prompt_commands_require_an_active_same_ecosystem_subagent() {
    let source = SourceKey::new("opencode.commands", "project").unwrap();
    let definition = |logical_id: &str| PromptCommandDefinition {
        id: SourceQualifiedCommandId::new(source.clone(), logical_id).unwrap(),
        name: logical_id.to_string(),
        description: logical_id.to_string(),
        template: "Review changes".to_string(),
        shell_preference: None,
        execution_target: PromptCommandExecutionTarget::FreshExternalSubagent {
            ecosystem_id: EcosystemId::new("opencode").unwrap(),
            logical_id: logical_id.to_string(),
        },
        availability: PromptCommandAvailability::Available,
        content_version: "command-v1".to_string(),
    };
    let mut commands = vec![
        PromptCommandCatalogEntry {
            definition: definition("reviewer"),
        },
        PromptCommandCatalogEntry {
            definition: definition("missing"),
        },
    ];
    let active = BTreeSet::from([(
        EcosystemId::new("opencode").unwrap(),
        "reviewer".to_string(),
    )]);
    let missing_candidate_id = commands[1].definition.id.stable_key();
    let mut conflicts = vec![PromptCommandConflict {
        conflict_key: "prompt-command-conflict".to_string(),
        command_name: "missing".to_string(),
        candidates: vec![PromptCommandConflictCandidate {
            candidate_id: missing_candidate_id.clone(),
            source: source.clone(),
            source_display_name: "OpenCode".to_string(),
            ecosystem_id: EcosystemId::new("opencode").unwrap(),
            content_version: "command-v1".to_string(),
            command_description: "missing".to_string(),
            source_scope: ExternalSourceScope::Project,
            source_location: ".opencode/commands/missing.md".to_string(),
            execution_target: commands[1].definition.execution_target.clone(),
            availability: PromptCommandAvailability::Available,
        }],
        selected_candidate_id: Some(missing_candidate_id),
    }];

    restrict_prompt_commands_without_active_subagents(&mut commands, &mut conflicts, &active);

    assert_eq!(
        commands[0].definition.availability,
        PromptCommandAvailability::Available
    );
    let PromptCommandAvailability::Restricted {
        required_capabilities,
        ..
    } = &commands[1].definition.availability
    else {
        panic!("missing delegated subagent must restrict the command");
    };
    assert_eq!(required_capabilities, &["command.external_subagent"]);
    assert!(matches!(
        conflicts[0].candidates[0].availability,
        PromptCommandAvailability::Restricted { .. }
    ));
    assert_eq!(conflicts[0].selected_candidate_id, None);
}

#[test]
fn native_prompt_command_choice_does_not_mark_external_candidate_as_self_conflicted() {
    let mut config = ExternalSourcesConfig::default();
    let native = "openbitfun.desktop:action:review".to_string();
    let external = "opencode.commands:project:review";
    let first_key = native_prompt_command_conflict_key(
        "local-user",
        "review",
        [(&native[..], "native-v1"), (external, "external-v1")],
    );

    assert!(reconcile_native_prompt_command_conflict_preference(
        &mut config,
        &first_key,
        external,
        std::slice::from_ref(&native),
    ));
    assert_eq!(
        config.conflict_choices.get(&first_key).map(String::as_str),
        Some(external)
    );
    assert!(config.conflicted_candidate_ids.contains(&native));
    assert!(!config.conflicted_candidate_ids.contains(external));

    let next_key = native_prompt_command_conflict_key(
        "local-user",
        "review",
        [(&native[..], "native-v1"), (external, "external-v2")],
    );
    assert!(reconcile_native_prompt_command_conflict_preference(
        &mut config,
        &next_key,
        &native,
        std::slice::from_ref(&native),
    ));
    assert!(!config.conflict_choices.contains_key(&first_key));
    assert_eq!(config.conflict_choices.get(&next_key), Some(&native));

    assert!(reconcile_native_prompt_command_reconfirmation(
        &mut config,
        "review",
        std::slice::from_ref(&native),
    ));
    assert!(!config.conflicted_candidate_ids.contains(&native));
    assert!(!config.conflict_choices.contains_key(&next_key));

    config.preference_revision = 7;
    config
        .conflict_choices
        .insert(first_key.clone(), external.to_string());
    assert!(native_prompt_command_expansion_guard_matches(
        &config, &first_key, external, 7,
    ));
    assert!(!native_prompt_command_expansion_guard_matches(
        &config, &first_key, external, 8,
    ));
    assert!(!native_prompt_command_expansion_guard_matches(
        &config,
        "prompt_command:local-user:review:first",
        external,
        7,
    ));
    let projection = NativePromptCommandConflictProjection {
        command_name: "review".to_string(),
        external_candidate_id: external.to_string(),
        conflict_key: first_key.clone(),
        selected_candidate_id: Some(external.to_string()),
    };
    assert!(validate_native_prompt_command_expansion_guard(
        &[&projection],
        &config,
        None,
        None,
        None,
    )
    .is_err());
    assert!(validate_native_prompt_command_expansion_guard(
        &[&projection],
        &config,
        Some(external),
        Some(&first_key),
        Some(7),
    )
    .is_ok());
    assert!(native_prompt_command_conflict_key_command(&first_key)
        .is_some_and(|command| command.eq_ignore_ascii_case("REVIEW")));
    assert_ne!(
        native_prompt_command_conflict_key_command(&first_key),
        Some("other")
    );

    let namespaced_key = native_prompt_command_conflict_key(
        "local-user",
        "foo:bar",
        [(&native[..], "native-v1"), (external, "external-v1")],
    );
    assert_eq!(
        native_prompt_command_conflict_key_command(&namespaced_key),
        Some("foo:bar")
    );
    assert_ne!(
        native_prompt_command_conflict_key_command(&namespaced_key),
        Some("bar")
    );
}

#[test]
fn native_prompt_command_reconfirmation_clears_the_whole_native_command_group() {
    let mut config = ExternalSourcesConfig::default();
    let first = "openbitfun.desktop:action:review".to_string();
    let second = "openbitfun.desktop:mode:review".to_string();
    let external = "opencode.commands:project:review";
    let desktop_key = native_prompt_command_conflict_key(
        "local-user",
        "review",
        [
            (&first[..], "native-v1"),
            (&second[..], "native-v1"),
            (external, "external-v1"),
        ],
    );
    let cli = "openbitfun.cli:action:review".to_string();
    let cli_key = native_prompt_command_conflict_key(
        "local-user",
        "review",
        [(&cli[..], "native-v1"), (external, "external-v1")],
    );
    config
        .conflicted_candidate_ids
        .extend([first.clone(), second.clone()]);
    config
        .conflict_choices
        .insert(desktop_key.clone(), external.to_string());
    config
        .conflict_choices
        .insert(cli_key.clone(), external.to_string());

    assert!(reconcile_native_prompt_command_reconfirmation(
        &mut config,
        "review",
        &[first.clone(), second.clone()],
    ));
    assert!(!config.conflicted_candidate_ids.contains(&first));
    assert!(!config.conflicted_candidate_ids.contains(&second));
    assert!(!config.conflict_choices.contains_key(&desktop_key));
    assert_eq!(
        config.conflict_choices.get(&cli_key).map(String::as_str),
        Some(external)
    );
}

#[tokio::test]
async fn finalizes_prompt_command_files_without_changing_plain_commands() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("src")).unwrap();
    std::fs::write(
        workspace.path().join("src/lib.rs"),
        "pub fn answer() -> u8 { 42 }\n``` embedded",
    )
    .unwrap();
    std::fs::write(workspace.path().join("README.md"), "Read me").unwrap();

    let plain = finalize_prompt_command_expansion(
        None,
        PromptCommandExpansion {
            content: "plain prompt".to_string(),
            workspace_file_references: Vec::new(),
            shell: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(plain.content, "plain prompt");

    let expanded = finalize_prompt_command_expansion(
        Some(workspace.path()),
        PromptCommandExpansion {
            content: "Review @src/lib.rs and @README.md".to_string(),
            workspace_file_references: vec![
                "src/lib.rs".to_string(),
                "README.md".to_string(),
                "src/lib.rs".to_string(),
            ],
            shell: None,
        },
    )
    .await
    .unwrap();

    assert!(expanded
        .content
        .starts_with("Review @src/lib.rs and @README.md"));
    assert_eq!(expanded.content.matches("### `src/lib.rs`").count(), 1);
    assert_eq!(expanded.content.matches("### `README.md`").count(), 1);
    assert!(expanded.content.contains("````text\npub fn answer()"));
    assert!(expanded.content.contains("\n````"));
}

#[tokio::test]
async fn prompt_command_file_limits_and_failures_are_atomic() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("large.txt"),
        vec![b'x'; MAX_PROMPT_COMMAND_FILE_BYTES + 1],
    )
    .unwrap();
    for index in 0..=MAX_PROMPT_COMMAND_FILE_REFERENCES {
        std::fs::write(workspace.path().join(format!("{index}.txt")), "x").unwrap();
    }
    for index in 0..3 {
        std::fs::write(
            workspace.path().join(format!("total-{index}.txt")),
            vec![b'x'; 48 * 1024],
        )
        .unwrap();
    }

    let without_workspace = finalize_prompt_command_expansion(
        None,
        PromptCommandExpansion {
            content: "prompt".to_string(),
            workspace_file_references: vec!["0.txt".to_string()],
            shell: None,
        },
    )
    .await
    .unwrap_err();
    assert!(without_workspace.contains("requires a local workspace"));

    let too_many = finalize_prompt_command_expansion(
        Some(workspace.path()),
        PromptCommandExpansion {
            content: "prompt".to_string(),
            workspace_file_references: (0..=MAX_PROMPT_COMMAND_FILE_REFERENCES)
                .map(|index| format!("{index}.txt"))
                .collect(),
            shell: None,
        },
    )
    .await
    .unwrap_err();
    assert!(too_many.contains("at most 8"));

    let oversized = finalize_prompt_command_expansion(
        Some(workspace.path()),
        PromptCommandExpansion {
            content: "prompt".to_string(),
            workspace_file_references: vec!["large.txt".to_string()],
            shell: None,
        },
    )
    .await
    .unwrap_err();
    assert!(oversized.contains("65536 byte limit"));

    let total_oversized = finalize_prompt_command_expansion(
        Some(workspace.path()),
        PromptCommandExpansion {
            content: "prompt".to_string(),
            workspace_file_references: (0..3).map(|index| format!("total-{index}.txt")).collect(),
            shell: None,
        },
    )
    .await
    .unwrap_err();
    assert!(total_oversized.contains("131072 byte total limit"));

    let final_oversized = finalize_prompt_command_expansion(
        Some(workspace.path()),
        PromptCommandExpansion {
            content: "x".repeat(MAX_EXPANDED_PROMPT_COMMAND_BYTES),
            workspace_file_references: vec!["0.txt".to_string()],
            shell: None,
        },
    )
    .await
    .unwrap_err();
    assert!(final_oversized.contains("1048576 byte limit"));

    let plain_oversized = finalize_prompt_command_expansion(
        None,
        PromptCommandExpansion {
            content: "x".repeat(MAX_EXPANDED_PROMPT_COMMAND_BYTES + 1),
            workspace_file_references: Vec::new(),
            shell: None,
        },
    )
    .await
    .unwrap_err();
    assert!(plain_oversized.contains("1048576 byte limit"));

    let one_bad_file = finalize_prompt_command_expansion(
        Some(workspace.path()),
        PromptCommandExpansion {
            content: "must not be returned partially".to_string(),
            workspace_file_references: vec!["0.txt".to_string(), "missing.txt".to_string()],
            shell: None,
        },
    )
    .await
    .unwrap_err();
    assert!(one_bad_file.contains("missing.txt"));
    assert!(!one_bad_file.contains("must not be returned partially"));
}
