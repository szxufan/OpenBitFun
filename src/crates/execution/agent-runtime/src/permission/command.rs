//! AST-backed command rules. Raw shell source is never wildcard-allowed by accident.
use super::*;
use openbitfun_agent_tools::command_permission::command_pattern_matches;

pub(super) fn command_effect(
    intent: &PermissionIntent,
    policy: &ResolvedPermissionPolicy,
    grants: &[PermissionGrant],
    case: PermissionResourceCaseSensitivity,
) -> PermissionEffect {
    let plan = intent.command.as_ref();
    let complete = plan.is_some_and(|p| p.complete && !p.commands.is_empty());
    let source = intent.resources.first().map(String::as_str).unwrap_or("");
    if source.is_empty() || intent.resources.len() != 1 {
        return PermissionEffect::Ask;
    }
    let commands: Vec<_> = plan
        .map(|p| p.commands.iter().collect())
        .unwrap_or_default();
    let mut result = PermissionEffect::Allow;
    // An unsupported program still supports deliberate exact/full-access approvals.
    let count = if complete {
        commands.len()
    } else {
        commands.len() + 1
    };
    for index in 0..count {
        let command = commands.get(index).copied();
        let read_only = complete && command.is_some_and(|c| c.read_only);
        let argv = command.map(|c| c.argv.as_slice());
        let powershell =
            plan.is_some_and(|p| matches!(p.shell_kind.as_str(), "pwsh" | "powershell"));
        let evaluate = |rules: &[openbitfun_runtime_ports::PermissionRule], default| {
            rules
                .iter()
                .rev()
                .find(|rule| {
                    let action_matches = wildcard_matches(
                        "process.execute",
                        &rule.action,
                        PermissionResourceCaseSensitivity::Sensitive,
                    ) || rule.action == "bash"
                        || (read_only
                            && wildcard_matches(
                                "process.execute.read_only",
                                &rule.action,
                                PermissionResourceCaseSensitivity::Sensitive,
                            ));
                    action_matches
                        && (rule.resource == "*"
                            || (rule.resource == source
                                && (rule.effect != PermissionEffect::Allow
                                    || !rule.resource.contains(['*', '?'])))
                            || ((complete || rule.effect != PermissionEffect::Allow)
                                && argv.is_some_and(|argv| {
                                    command_pattern_matches(&rule.resource, argv, powershell)
                                }))
                            || (rule.effect != PermissionEffect::Allow
                                && wildcard_matches(source, &rule.resource, case)))
                })
                .map(|rule| rule.effect)
                .unwrap_or(default)
        };
        let mut effect = evaluate(policy.rules(), PermissionEffect::Ask);
        for layer in policy.constraint_layers() {
            effect = effect.most_restrictive(evaluate(layer.rules(), PermissionEffect::Allow));
        }
        if effect == PermissionEffect::Deny {
            return effect;
        }
        if effect == PermissionEffect::Ask {
            let remembered = grants.iter().any(|grant| {
                (grant.action == "process.execute" && grant.resource.starts_with("exec-plan:") && intent.save_resources.contains(&grant.resource))
                    // Existing Bash grants remain exact command approvals, never patterns.
                    || (grant.action == "bash" && grant.resource == source)
            });
            if !remembered {
                result = PermissionEffect::Ask;
            }
        }
    }
    result
}
