use super::super::*;
use openbitfun_product_domains::external_sources::{
    PromptCommandShellExpansion, PromptCommandShellInvocation, PromptCommandShellPreference,
};

fn prompt_shell_expansion(command: &str, can_remember: bool) -> PromptCommandExpansion {
    let content = format!("Before !`{command}` after");
    PromptCommandExpansion {
        content: content.clone(),
        workspace_file_references: Vec::new(),
        shell: Some(PromptCommandShellExpansion {
            working_directory: std::env::current_dir()
                .expect("the test process should have an absolute working directory"),
            preference: PromptCommandShellPreference::HostDefault,
            invocations: vec![PromptCommandShellInvocation {
                range_start: 7,
                range_end: content.len() - 6,
                command: command.to_string(),
                can_remember,
            }],
        }),
    }
}

fn resolved_test_shell() -> ResolvedPromptCommandShell {
    ResolvedPromptCommandShell {
        display_name: "Bash".to_string(),
        path: PathBuf::from("/bin/bash"),
        kind: tool_runtime::exec_command::ExecCommandShellKind::Bash,
    }
}

#[test]
fn prompt_shell_review_fingerprint_covers_command_cwd_shell_and_content_version() {
    let plan = prepare_prompt_command_shell_plan(
        prompt_shell_expansion("git status", true),
        "OpenCode",
        "local-user",
        "candidate-v1",
        "content-v1",
        4,
        resolved_test_shell(),
    )
    .unwrap();
    assert!(plan.review.can_remember);
    assert_eq!(plan.review.commands, ["git status"]);
    assert_eq!(plan.review.shell_executable, "/bin/bash");

    let changed_command = prepare_prompt_command_shell_plan(
        prompt_shell_expansion("git diff", true),
        "OpenCode",
        "local-user",
        "candidate-v1",
        "content-v1",
        4,
        resolved_test_shell(),
    )
    .unwrap();
    let changed_content = prepare_prompt_command_shell_plan(
        prompt_shell_expansion("git status", true),
        "OpenCode",
        "local-user",
        "candidate-v1",
        "content-v2",
        4,
        resolved_test_shell(),
    )
    .unwrap();

    assert_ne!(
        plan.review.plan_fingerprint,
        changed_command.review.plan_fingerprint
    );
    assert_ne!(
        plan.review.plan_fingerprint,
        changed_content.review.plan_fingerprint
    );
}

#[test]
fn prompt_shell_preference_falls_back_only_when_the_ecosystem_allows_it() {
    let host = resolve_prompt_command_shell(&PromptCommandShellPreference::HostDefault).unwrap();
    let preferred = resolve_prompt_command_shell(&PromptCommandShellPreference::Preferred {
        executable: "openbitfun-missing-shell-for-test".to_string(),
    })
    .unwrap();

    assert_eq!(preferred.path, host.path);
    assert!(
        resolve_prompt_command_shell(&PromptCommandShellPreference::Required {
            executable: "openbitfun-missing-shell-for-test".to_string(),
        })
        .is_err()
    );
    let required_one_of =
        resolve_prompt_command_shell(&PromptCommandShellPreference::RequiredOneOf {
            executables: vec![
                "openbitfun-missing-shell-for-test".to_string(),
                host.path.to_string_lossy().to_string(),
            ],
        })
        .unwrap();
    assert_eq!(required_one_of.path, host.path);
}

#[test]
fn prompt_shell_executable_is_canonical_before_review_and_execution() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("reviewed-shell");
    std::fs::write(&executable, "test").unwrap();

    let resolved = finalize_resolved_prompt_command_shell(
        "Reviewed shell".to_string(),
        executable.clone(),
        ShellType::Custom("reviewed-shell".to_string()),
    )
    .unwrap();

    assert!(resolved.path.is_absolute());
    assert_eq!(resolved.path, dunce::canonicalize(executable).unwrap());
}

#[cfg(unix)]
#[test]
fn prompt_shell_fingerprint_keeps_non_utf8_path_identity() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let first = PathBuf::from(OsString::from_vec(b"/tmp/openbitfun-\x80".to_vec()));
    let second = PathBuf::from(OsString::from_vec(b"/tmp/openbitfun-\x81".to_vec()));
    assert_ne!(
        prompt_command_shell_path_bytes(&first),
        prompt_command_shell_path_bytes(&second)
    );
}

#[cfg(windows)]
#[test]
fn prompt_shell_fingerprint_keeps_non_unicode_windows_path_identity() {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    let first = PathBuf::from(OsString::from_wide(&[b'C' as u16, b':' as u16, 0xd800]));
    let second = PathBuf::from(OsString::from_wide(&[b'C' as u16, b':' as u16, 0xd801]));
    assert_ne!(
        prompt_command_shell_path_bytes(&first),
        prompt_command_shell_path_bytes(&second)
    );
}

#[test]
fn dynamic_shell_plans_cannot_be_remembered_and_outputs_replace_in_template_order() {
    let mut expansion = prompt_shell_expansion("first $ARGUMENTS", false);
    let second_start = expansion.content.len();
    expansion.content.push_str(" then !`second`");
    expansion
        .shell
        .as_mut()
        .unwrap()
        .invocations
        .push(PromptCommandShellInvocation {
            range_start: second_start + 6,
            range_end: expansion.content.len(),
            command: "second".to_string(),
            can_remember: true,
        });
    let plan = prepare_prompt_command_shell_plan(
        expansion,
        "Claude Code",
        "local-user",
        "candidate-v1",
        "content-v1",
        1,
        resolved_test_shell(),
    )
    .unwrap();

    assert!(!plan.review.can_remember);
    assert_eq!(
        apply_prompt_command_shell_outputs(
            &plan.expansion.content,
            &plan.invocations,
            &["one".to_string(), "two".to_string()],
        )
        .unwrap(),
        "Before one after then two"
    );
}

#[tokio::test]
async fn prompt_shell_plan_uses_stdout_even_after_nonzero_exit_and_preserves_template_order() {
    let resolved = resolve_prompt_command_shell(&PromptCommandShellPreference::HostDefault)
        .expect("the host default shell should resolve");
    let (first, second) = match resolved.kind {
        ExecCommandShellKind::PowerShell | ExecCommandShellKind::PowerShellCore => (
            "Write-Output first; [Console]::Error.WriteLine('not-prompt'); exit 7",
            "Write-Output second",
        ),
        ExecCommandShellKind::Cmd => (
            "echo first & echo not-prompt 1>&2 & exit /b 7",
            "echo second",
        ),
        _ => (
            "printf first; printf not-prompt >&2; exit 7",
            "printf second",
        ),
    };
    let content = format!("Before !`{first}` middle !`{second}` after");
    let invocations = [first, second]
        .into_iter()
        .map(|command| {
            let marker = format!("!`{command}`");
            let range_start = content.find(&marker).unwrap();
            PromptCommandShellInvocation {
                range_start,
                range_end: range_start + marker.len(),
                command: command.to_string(),
                can_remember: true,
            }
        })
        .collect();
    let expansion = PromptCommandExpansion {
        content,
        workspace_file_references: Vec::new(),
        shell: Some(PromptCommandShellExpansion {
            working_directory: tempfile::tempdir().unwrap().keep(),
            preference: PromptCommandShellPreference::HostDefault,
            invocations,
        }),
    };
    let plan = prepare_prompt_command_shell_plan(
        expansion,
        "OpenCode",
        "local-user",
        "candidate-v1",
        "content-v1",
        1,
        resolved,
    )
    .unwrap();

    let expanded = execute_prompt_command_shell_plan(plan).await.unwrap();

    assert!(!expanded.content.contains("not-prompt"));
    let first_index = expanded.content.find("first").unwrap();
    let second_index = expanded.content.find("second").unwrap();
    assert!(first_index < second_index);
    assert!(expanded.content.starts_with("Before "));
    assert!(expanded.content.ends_with(" after"));
}
