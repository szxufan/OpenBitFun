//! Execution-host command facts. These are never model-supplied arguments.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionCommand {
    pub argv: Vec<String>,
    /// A conservative effect classification under the host's trusted shell environment.
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CommandPermissionPlan {
    pub shell_kind: String,
    pub shell_path: String,
    pub workdir: String,
    pub complete: bool,
    pub commands: Vec<PermissionCommand>,
}

/// Rules use a small shell-independent argument-pattern language. Only a final
/// standalone `*` spans multiple arguments; operators never become arguments.
pub fn command_pattern_matches(pattern: &str, argv: &[String], powershell: bool) -> bool {
    let Some(words) = pattern_words(pattern) else {
        return false;
    };
    let tail = words.last().is_some_and(|word| word == "*");
    let fixed = words.len().saturating_sub(usize::from(tail));
    if argv.len() < fixed || (!tail && argv.len() != fixed) {
        return false;
    }
    words
        .iter()
        .take(fixed)
        .zip(argv)
        .enumerate()
        .all(|(index, (pattern, value))| {
            if powershell && index == 0 {
                token_glob(&value.to_ascii_lowercase(), &pattern.to_ascii_lowercase())
            } else {
                token_glob(value, pattern)
            }
        })
}

fn pattern_words(pattern: &str) -> Option<Vec<String>> {
    if pattern.len() > 32_768 {
        return None;
    }
    let mut result = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut active = false;
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                active = true;
            }
            None if ";|&<>()`\n\r\0".contains(c) => return None,
            None if c.is_whitespace() => {
                if active {
                    result.push(std::mem::take(&mut word));
                    active = false;
                }
            }
            None if c == '\\'
                && chars.peek().is_some_and(|next| {
                    next.is_whitespace() || matches!(next, '\'' | '"' | '\\')
                }) =>
            {
                word.push(chars.next()?);
                active = true;
            }
            None => {
                word.push(c);
                active = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if active {
        result.push(word);
    }
    (!result.is_empty()).then_some(result)
}

fn token_glob(value: &str, pattern: &str) -> bool {
    if value.len().saturating_mul(pattern.len()) > 1_048_576 {
        return false;
    }
    // Linear-space wildcard matching without path-separator normalization.
    let input: Vec<char> = value.chars().collect();
    let mut previous = vec![false; input.len() + 1];
    previous[0] = true;
    for p in pattern.chars() {
        let mut next = vec![false; input.len() + 1];
        if p == '*' {
            next[0] = previous[0];
        }
        for (i, c) in input.iter().enumerate() {
            next[i + 1] = match p {
                '*' => previous[i + 1] || next[i],
                '?' => previous[i],
                _ => previous[i] && p == *c,
            };
        }
        previous = next;
    }
    previous[input.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn matches(pattern: &str, args: &[&str]) -> bool {
        command_pattern_matches(
            pattern,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            false,
        )
    }
    #[test]
    fn command_rules_respect_argument_and_operator_boundaries() {
        assert!(matches(
            "git clone *",
            &["git", "clone", "url with spaces", "target"]
        ));
        assert!(matches(
            "git clone 'url with spaces'",
            &["git", "clone", "url with spaces"]
        ));
        assert!(!matches("git clone*", &["git", "clone", "repo"]));
        assert!(!matches("git clone *; rm *", &["git", "clone", "repo"]));
        assert!(!matches("git\nclone *", &["git", "clone", "repo"]));
        assert!(!matches("git clone *", &["git", "status"]));
        assert!(matches(
            "git clone *",
            &["git", "clone", "repo; literal argument"]
        ));
        assert!(!matches(
            "git clone repo",
            &["git", "clone", "repo", "other"]
        ));
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    #[test]
    fn legacy_intents_round_trip_without_command_evidence() {
        let payload = serde_json::json!({"action":"bash","resources":["git status"],"saveResources":["git status"]});
        let intent: crate::PermissionIntent = serde_json::from_value(payload.clone()).unwrap();
        assert!(intent.command.is_none());
        assert_eq!(serde_json::to_value(intent).unwrap(), payload);
        let incomplete: CommandPermissionPlan =
            serde_json::from_value(serde_json::json!({"shellKind":"bash"})).unwrap();
        assert!(!incomplete.complete);
        assert!(incomplete.commands.is_empty());
    }
}
